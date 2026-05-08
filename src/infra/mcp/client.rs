use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;

use crate::config::model::AppConfig;
use crate::domain::chat::models::{McpExposureMode, McpOverrides};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_CLIENT_NAME: &str = "auto-claude-code-rs";
const MCP_CLIENT_VERSION: &str = "1.0.0";

#[derive(Clone)]
pub struct McpClient {
    http: reqwest::Client,
    config: McpClientConfig,
}

#[derive(Clone)]
struct McpClientConfig {
    config_path: String,
    base_urls: Vec<String>,
}

#[derive(Clone)]
struct McpEndpointClient {
    http: reqwest::Client,
    base_url: String,
    session_id: Arc<Mutex<Option<String>>>,
    init_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone)]
struct NamedToolSchema {
    full_name: String,
    schema: Value,
}

#[derive(Debug, Clone)]
pub struct McpServerPreview {
    pub endpoint: String,
    pub endpoint_key: String,
    pub mode: McpExposureMode,
    pub ok: bool,
    pub tool_count: usize,
    pub tools: Vec<McpToolPreview>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpToolPreview {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct LazyToolPreview {
    pub endpoint: String,
    pub endpoint_key: String,
    pub tool_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct McpToolSelection {
    pub activated_endpoint_keys: Vec<String>,
    pub activated_tools_by_endpoint: HashMap<String, Vec<String>>,
}

impl McpToolSelection {
    pub fn activate(&mut self, endpoint_key: &str, tool_names: &[String]) {
        if !self
            .activated_endpoint_keys
            .iter()
            .any(|item| item == endpoint_key)
        {
            self.activated_endpoint_keys.push(endpoint_key.to_string());
        }
        let entry = self
            .activated_tools_by_endpoint
            .entry(endpoint_key.to_string())
            .or_default();
        for tool_name in tool_names {
            if !entry.iter().any(|item| item == tool_name) {
                entry.push(tool_name.clone());
            }
        }
    }
}

impl McpClient {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let http = build_http_client(config.mcp.timeout, config.mcp.connect_timeout)?;
        Ok(Self {
            http,
            config: McpClientConfig {
                config_path: config.mcp.config_path.clone(),
                base_urls: configured_base_urls(config),
            },
        })
    }

    pub async fn list_tool_schemas(
        &self,
        overrides: Option<&McpOverrides>,
        selection: Option<&McpToolSelection>,
    ) -> Result<Vec<Value>> {
        let endpoints = self.endpoint_clients(overrides)?;
        if endpoints.is_empty() {
            bail!("no MCP endpoints configured");
        }

        let mut merged = Vec::new();
        let mut seen = HashSet::new();
        let mut errors = Vec::new();
        let mut inspected_any = false;

        for endpoint in endpoints {
            if !should_expose_endpoint_tools(&endpoint.base_url, overrides, selection) {
                continue;
            }
            inspected_any = true;
            match endpoint.list_tool_schemas().await {
                Ok(schemas) => {
                    for schema in schemas {
                        if !should_include_tool_schema(
                            &endpoint.base_url,
                            &schema.full_name,
                            overrides,
                            selection,
                        ) {
                            continue;
                        }
                        if seen.insert(schema.full_name.clone()) {
                            merged.push(schema.schema);
                        }
                    }
                }
                Err(error) => errors.push(format!("{}: {error}", endpoint.base_url)),
            }
        }

        if !inspected_any {
            return Ok(Vec::new());
        }

        if merged.is_empty() {
            if errors.is_empty() {
                bail!("no MCP tools available");
            }
            bail!(errors.join(" | "));
        }

        Ok(merged)
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Value,
        overrides: Option<&McpOverrides>,
        selection: Option<&McpToolSelection>,
    ) -> String {
        match self
            .call_tool_inner(tool_name, arguments, overrides, selection)
            .await
        {
            Ok(output) => output,
            Err(error) => format!("[MCP Error] {error}"),
        }
    }

    pub async fn inspect_servers(
        &self,
        overrides: Option<&McpOverrides>,
    ) -> Result<Vec<McpServerPreview>> {
        let endpoints = self.endpoint_clients(overrides)?;
        if endpoints.is_empty() {
            return Ok(Vec::new());
        }

        let mut previews = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            match endpoint.list_raw_tools().await {
                Ok(raw_tools) => {
                    let tools = raw_tools
                        .into_iter()
                        .filter_map(|tool| build_tool_preview(&tool))
                        .collect::<Vec<_>>();
                    previews.push(McpServerPreview {
                        endpoint: endpoint.base_url.clone(),
                        endpoint_key: endpoint_key(&endpoint.base_url),
                        mode: overrides
                            .map(|value| value.exposure_mode_for_endpoint(&endpoint.base_url))
                            .unwrap_or(McpExposureMode::Eager),
                        ok: true,
                        tool_count: tools.len(),
                        tools,
                        error: None,
                    });
                }
                Err(error) => previews.push(McpServerPreview {
                    endpoint: endpoint.base_url.clone(),
                    endpoint_key: endpoint_key(&endpoint.base_url),
                    mode: overrides
                        .map(|value| value.exposure_mode_for_endpoint(&endpoint.base_url))
                        .unwrap_or(McpExposureMode::Eager),
                    ok: false,
                    tool_count: 0,
                    tools: Vec::new(),
                    error: Some(error.to_string()),
                }),
            }
        }

        Ok(previews)
    }

    pub async fn search_lazy_tool_candidates(
        &self,
        overrides: Option<&McpOverrides>,
        query: &str,
    ) -> Result<Vec<LazyToolPreview>> {
        let endpoints = self.endpoint_clients(overrides)?;
        let normalized_query = query.trim().to_ascii_lowercase();
        let mut matches = Vec::new();

        for endpoint in endpoints {
            let Some(overrides) = overrides else {
                continue;
            };
            if overrides.exposure_mode_for_endpoint(&endpoint.base_url) != McpExposureMode::Lazy {
                continue;
            }
            let endpoint_key = endpoint_key(&endpoint.base_url);
            let raw_tools = endpoint.list_raw_tools().await?;
            for tool in raw_tools {
                let Some(preview) = build_tool_preview(&tool) else {
                    continue;
                };
                let haystack = format!(
                    "{} {} {}",
                    endpoint.base_url, preview.name, preview.description
                )
                .to_ascii_lowercase();
                if !normalized_query.is_empty() && !haystack.contains(&normalized_query) {
                    continue;
                }
                matches.push(LazyToolPreview {
                    endpoint: endpoint.base_url.clone(),
                    endpoint_key: endpoint_key.clone(),
                    tool_name: preview.name,
                    description: preview.description,
                });
            }
        }

        Ok(matches)
    }

    async fn call_tool_inner(
        &self,
        tool_name: &str,
        arguments: Value,
        overrides: Option<&McpOverrides>,
        selection: Option<&McpToolSelection>,
    ) -> Result<String> {
        let endpoints = self.endpoint_clients(overrides)?;
        if endpoints.is_empty() {
            bail!("no MCP endpoints configured");
        }

        let raw_name = tool_name.strip_prefix("mcp_").unwrap_or(tool_name);
        let (target_key, actual_name) = split_prefixed_tool_name(raw_name);

        if let Some(target_key) = target_key {
            let endpoint = endpoints
                .into_iter()
                .find(|item| endpoint_key(&item.base_url) == target_key)
                .with_context(|| {
                    format!("MCP endpoint not found for tool {tool_name}: {target_key}")
                })?;
            if !should_expose_endpoint_tools(&endpoint.base_url, overrides, selection) {
                bail!("MCP endpoint not activated for tool {tool_name}: {target_key}");
            }
            return endpoint.call_tool(&actual_name, arguments).await;
        }

        let mut last_error = None;
        for endpoint in endpoints {
            if !should_expose_endpoint_tools(&endpoint.base_url, overrides, selection) {
                continue;
            }
            match endpoint.call_tool(&actual_name, arguments.clone()).await {
                Ok(output) => return Ok(output),
                Err(error) => last_error = Some(format!("{}: {error}", endpoint.base_url)),
            }
        }

        bail!(
            "{}",
            last_error.unwrap_or_else(|| format!("tool not available: {tool_name}"))
        )
    }

    fn endpoint_clients(&self, overrides: Option<&McpOverrides>) -> Result<Vec<McpEndpointClient>> {
        let base_urls = resolve_effective_base_urls(&self.config, overrides)?;
        base_urls
            .into_iter()
            .map(|base_url| {
                Ok(McpEndpointClient {
                    http: self.http.clone(),
                    base_url,
                    session_id: Arc::new(Mutex::new(None)),
                    init_lock: Arc::new(Mutex::new(())),
                })
            })
            .collect()
    }
}

impl McpEndpointClient {
    async fn list_raw_tools(&self) -> Result<Vec<Value>> {
        self.ensure_initialized().await?;
        let response = self.send_request("tools/list", json!({})).await?;
        if let Some(error) = response.get("error") {
            bail!("mcp tools/list error: {error}");
        }

        Ok(response
            .get("result")
            .and_then(|value| value.get("tools"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn list_tool_schemas(&self) -> Result<Vec<NamedToolSchema>> {
        Ok(self
            .list_raw_tools()
            .await?
            .into_iter()
            .filter_map(|tool| build_tool_schema(&self.base_url, &tool))
            .collect())
    }

    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String> {
        self.ensure_initialized().await?;
        let response = self
            .send_request(
                "tools/call",
                json!({
                    "name": tool_name,
                    "arguments": arguments
                }),
            )
            .await?;

        if let Some(error) = response.get("error") {
            bail!("mcp tools/call error: {error}");
        }

        let Some(content) = response
            .get("result")
            .and_then(|value| value.get("content"))
            .and_then(Value::as_array)
        else {
            return Ok("(no output)".to_string());
        };

        if content.is_empty() {
            return Ok("(no output)".to_string());
        }

        let parts = content
            .iter()
            .map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    serde_json::to_string(item).unwrap_or_else(|_| item.to_string())
                }
            })
            .collect::<Vec<_>>();

        Ok(parts.join("\n"))
    }

    async fn ensure_initialized(&self) -> Result<()> {
        if self.session_id.lock().await.is_some() {
            return Ok(());
        }

        let _guard = self.init_lock.lock().await;
        if self.session_id.lock().await.is_some() {
            return Ok(());
        }

        let response = self
            .send_request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": MCP_CLIENT_NAME,
                        "version": MCP_CLIENT_VERSION
                    }
                }),
            )
            .await?;

        if let Some(error) = response.get("error") {
            bail!("mcp initialize error: {error}");
        }

        Ok(())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        if self.base_url.is_empty() {
            bail!("mcp base_url is empty");
        }

        let session_id = self.session_id.lock().await.clone();
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(session_id) = session_id {
            headers.insert(
                "mcp-session-id",
                HeaderValue::from_str(&session_id).context("invalid mcp session id header")?,
            );
        }

        let response = self
            .http
            .post(&self.base_url)
            .headers(headers)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .with_context(|| format!("failed to call mcp method {method}"))?;

        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            *self.session_id.lock().await = Some(session_id.to_string());
        }

        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("failed to read mcp response body for {method}"))?;

        if !status.is_success() {
            bail!("mcp request failed with status {status}: {body}");
        }

        parse_response_body(&body)
            .with_context(|| format!("failed to parse mcp response for {method}"))
    }
}

fn build_http_client(timeout: u64, connect_timeout: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout))
        .connect_timeout(Duration::from_secs(connect_timeout))
        .build()
        .context("failed to build mcp reqwest client")
}

fn configured_base_urls(config: &AppConfig) -> Vec<String> {
    let mut urls = Vec::new();
    if !config.mcp.base_url.trim().is_empty() {
        urls.push(config.mcp.base_url.clone());
    }
    urls.extend(config.mcp.base_urls.clone());
    urls.extend(
        config
            .mcp
            .servers
            .iter()
            .map(|server| server.base_url.clone()),
    );
    unique_non_empty(urls)
}

fn resolve_effective_base_urls(
    config: &McpClientConfig,
    overrides: Option<&McpOverrides>,
) -> Result<Vec<String>> {
    let mut urls = config.base_urls.clone();

    if let Some(overrides) = overrides {
        urls.extend(overrides.base_urls.clone());
        if let Some(path) = overrides
            .config_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            urls.extend(load_mcp_urls_from_path(path)?);
        }
    } else if !config.config_path.trim().is_empty() {
        urls.extend(load_mcp_urls_from_path(config.config_path.trim())?);
    }

    let mut urls = unique_non_empty(urls);
    if let Some(overrides) = overrides
        && !overrides.disabled_urls.is_empty()
    {
        let disabled = overrides
            .disabled_urls
            .iter()
            .map(|item| item.trim().trim_end_matches('/').to_string())
            .filter(|item| !item.is_empty())
            .collect::<HashSet<_>>();
        urls.retain(|url| !disabled.contains(url));
    }

    Ok(urls)
}

fn load_mcp_urls_from_path(path: &str) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read MCP config path: {path}"))?;
    let json: Value =
        serde_json::from_str(&raw).with_context(|| format!("invalid MCP config json at {path}"))?;
    let urls = extract_mcp_urls(&json);
    if urls.is_empty() {
        bail!("MCP config path did not contain any server url entries: {path}");
    }
    Ok(urls)
}

fn should_expose_endpoint_tools(
    endpoint: &str,
    overrides: Option<&McpOverrides>,
    selection: Option<&McpToolSelection>,
) -> bool {
    let Some(overrides) = overrides else {
        return true;
    };
    match overrides.exposure_mode_for_endpoint(endpoint) {
        McpExposureMode::Disabled => false,
        McpExposureMode::Eager => true,
        McpExposureMode::Lazy => selection
            .map(|value| {
                value
                    .activated_endpoint_keys
                    .iter()
                    .any(|item| item == &endpoint_key(endpoint))
            })
            .unwrap_or(false),
    }
}

fn should_include_tool_schema(
    endpoint: &str,
    full_tool_name: &str,
    overrides: Option<&McpOverrides>,
    selection: Option<&McpToolSelection>,
) -> bool {
    let Some(overrides) = overrides else {
        return true;
    };
    match overrides.exposure_mode_for_endpoint(endpoint) {
        McpExposureMode::Disabled => false,
        McpExposureMode::Eager => true,
        McpExposureMode::Lazy => {
            let Some(selection) = selection else {
                return false;
            };
            if selection.activated_endpoint_keys.is_empty() {
                return false;
            }
            let current_key = endpoint_key(endpoint);
            if !selection
                .activated_endpoint_keys
                .iter()
                .any(|item| item == &current_key)
            {
                return false;
            }
            let Some(activated_names) = selection.activated_tools_by_endpoint.get(&current_key) else {
                return false;
            };
            if activated_names.is_empty() {
                return true;
            }
            let (_, actual_name) = split_prefixed_tool_name(
                full_tool_name.strip_prefix("mcp_").unwrap_or(full_tool_name),
            );
            activated_names.iter().any(|item| item == &actual_name)
        }
    }
}

fn extract_mcp_urls(json: &Value) -> Vec<String> {
    json.get("mcpServers")
        .or_else(|| json.get("servers"))
        .and_then(Value::as_object)
        .map(|servers| {
            servers
                .values()
                .filter_map(|server| {
                    server
                        .get("url")
                        .or_else(|| server.get("base_url"))
                        .or_else(|| server.get("baseUrl"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn unique_non_empty(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn build_tool_schema(base_url: &str, tool: &Value) -> Option<NamedToolSchema> {
    let name = tool.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }

    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parameters = tool
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let scoped_endpoint = endpoint_key(base_url);
    let full_name = format!("mcp_{scoped_endpoint}__{name}");

    Some(NamedToolSchema {
        full_name: full_name.clone(),
        schema: json!({
            "type": "function",
            "function": {
                "name": full_name,
                "description": format!("[MCP {}] {description}", base_url),
                "parameters": parameters
            }
        }),
    })
}

fn build_tool_preview(tool: &Value) -> Option<McpToolPreview> {
    let name = tool.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }

    Some(McpToolPreview {
        name: name.to_string(),
        description: tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn endpoint_key(base_url: &str) -> String {
    let normalized = base_url.trim().trim_end_matches('/');
    let readable = normalized
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let compact = readable
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let digest = format!("{:x}", Sha1::digest(normalized.as_bytes()));
    format!("{compact}_{}", &digest[..8])
}

fn split_prefixed_tool_name(raw_name: &str) -> (Option<String>, String) {
    let Some((encoded_endpoint, tool_name)) = raw_name.split_once("__") else {
        return (None, raw_name.to_string());
    };
    (Some(encoded_endpoint.to_string()), tool_name.to_string())
}

fn parse_response_body(body: &str) -> Result<Value> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    let mut data_lines = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim().to_string());
        }
    }

    if let Some(last) = data_lines.last() {
        return serde_json::from_str(last).context("invalid json in final sse data line");
    }

    bail!("mcp response did not contain json or sse data");
}

#[cfg(test)]
mod tests {
    use super::{
        McpClient, McpOverrides, build_tool_schema, configured_base_urls, extract_mcp_urls,
        parse_response_body, resolve_effective_base_urls,
    };
    use crate::config::model::{AppConfig, McpServerConfig};
    use axum::Router;
    use axum::extract::Json as AxumJson;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::routing::post;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    #[test]
    fn parses_plain_json_response() {
        let value = parse_response_body(r#"{"jsonrpc":"2.0","result":{"ok":true}}"#).unwrap();
        assert_eq!(value["result"]["ok"], json!(true));
    }

    #[test]
    fn parses_sse_response() {
        let value = parse_response_body(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"result\":{\"ok\":true}}\n\n",
        )
        .unwrap();
        assert_eq!(value["result"]["ok"], json!(true));
    }

    #[test]
    fn extracts_urls_from_mcp_config_json() {
        let urls = extract_mcp_urls(&json!({
            "mcpServers": {
                "one": { "url": "http://a.example/mcp" },
                "two": { "baseUrl": "http://b.example/mcp" }
            }
        }));
        assert_eq!(urls, vec!["http://a.example/mcp", "http://b.example/mcp"]);
    }

    #[test]
    fn merges_multiple_configured_sources() {
        let mut config = AppConfig::default();
        config.mcp.base_url = "http://one.example/mcp".to_string();
        config.mcp.base_urls = vec!["http://two.example/mcp".to_string()];
        config.mcp.servers = vec![McpServerConfig {
            name: Some("three".to_string()),
            base_url: "http://three.example/mcp".to_string(),
        }];

        assert_eq!(
            configured_base_urls(&config),
            vec![
                "http://one.example/mcp",
                "http://two.example/mcp",
                "http://three.example/mcp"
            ]
        );
    }

    #[test]
    fn resolves_override_urls_without_config_path() {
        let config = super::McpClientConfig {
            config_path: String::new(),
            base_urls: vec!["http://one.example/mcp".to_string()],
        };
        let overrides = McpOverrides {
            config_path: None,
            base_urls: vec!["http://two.example/mcp".to_string()],
            disabled_urls: Vec::new(),
            lazy_urls: Vec::new(),
        };

        assert_eq!(
            resolve_effective_base_urls(&config, Some(&overrides)).unwrap(),
            vec!["http://one.example/mcp", "http://two.example/mcp"]
        );
    }

    #[test]
    fn filters_disabled_urls_from_effective_set() {
        let config = super::McpClientConfig {
            config_path: String::new(),
            base_urls: vec![
                "http://one.example/mcp".to_string(),
                "http://two.example/mcp/".to_string(),
            ],
        };
        let overrides = McpOverrides {
            config_path: None,
            base_urls: vec!["http://three.example/mcp".to_string()],
            disabled_urls: vec![
                "http://two.example/mcp".to_string(),
                "http://three.example/mcp/".to_string(),
            ],
            lazy_urls: Vec::new(),
        };

        assert_eq!(
            resolve_effective_base_urls(&config, Some(&overrides)).unwrap(),
            vec!["http://one.example/mcp"]
        );
    }

    #[test]
    fn converts_mcp_tools_to_endpoint_scoped_openai_function_schema() {
        let schema = build_tool_schema(
            "http://mcp.example/mcp",
            &json!({
                "name": "read_file",
                "description": "Read a file via MCP",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }
            }),
        )
        .unwrap();

        let name = schema.schema["function"]["name"].as_str().unwrap();
        assert!(name.starts_with("mcp_"));
        assert!(name.ends_with("__read_file"));
        assert_eq!(
            schema.schema["function"]["parameters"]["required"],
            json!(["path"])
        );
    }

    #[tokio::test]
    async fn degrades_gracefully_when_all_mcp_servers_fail() {
        async fn fail_mcp() -> (StatusCode, &'static str) {
            (StatusCode::INTERNAL_SERVER_ERROR, "mcp offline")
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let app = Router::new().route("/", post(fail_mcp));
            axum::serve(listener, app).await.unwrap();
        });

        let mut config = AppConfig::default();
        config.mcp.base_url = format!("http://{addr}");
        config.mcp.base_urls = vec![format!("http://{addr}")];
        config.mcp.connect_timeout = 1;
        config.mcp.timeout = 1;
        let client = McpClient::new(&config).unwrap();

        let list_err = client
            .list_tool_schemas(None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(list_err.contains("500 Internal Server Error"));

        let tool_output = client
            .call_tool("mcp_read_file", json!({"path": "foo"}), None, None)
            .await;
        assert!(tool_output.starts_with("[MCP Error]"));
        assert!(tool_output.contains("500 Internal Server Error"));

        server.abort();
    }

    #[tokio::test]
    async fn merges_tools_from_multiple_mcp_servers() {
        async fn mcp_stub(
            headers: HeaderMap,
            AxumJson(payload): AxumJson<Value>,
        ) -> (HeaderMap, AxumJson<Value>) {
            let mut response_headers = HeaderMap::new();
            if !headers.contains_key("mcp-session-id") {
                response_headers.insert("mcp-session-id", HeaderValue::from_static("demo-session"));
            }

            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = match method {
                "initialize" => json!({ "jsonrpc": "2.0", "result": { "capabilities": {} } }),
                "tools/list" => json!({
                    "jsonrpc": "2.0",
                    "result": {
                        "tools": [{
                            "name": "echo",
                            "description": "Echo from stub",
                            "inputSchema": { "type": "object", "properties": {} }
                        }]
                    }
                }),
                "tools/call" => json!({
                    "jsonrpc": "2.0",
                    "result": {
                        "content": [{ "type": "text", "text": "ok" }]
                    }
                }),
                _ => json!({ "jsonrpc": "2.0", "error": { "message": "unknown" } }),
            };
            (response_headers, AxumJson(body))
        }

        let listener_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_a = listener_a.local_addr().unwrap();
        let server_a = tokio::spawn(async move {
            let app = Router::new().route("/", post(mcp_stub));
            axum::serve(listener_a, app).await.unwrap();
        });

        let listener_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_b = listener_b.local_addr().unwrap();
        let server_b = tokio::spawn(async move {
            let app = Router::new().route("/", post(mcp_stub));
            axum::serve(listener_b, app).await.unwrap();
        });

        let mut config = AppConfig::default();
        config.mcp.base_url = format!("http://{addr_a}");
        config.mcp.base_urls = vec![format!("http://{addr_b}")];
        config.mcp.connect_timeout = 1;
        config.mcp.timeout = 1;
        let client = McpClient::new(&config).unwrap();

        let tools = client.list_tool_schemas(None, None).await.unwrap();
        assert_eq!(tools.len(), 2);

        server_a.abort();
        server_b.abort();
    }
}
