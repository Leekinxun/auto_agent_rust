use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::config::model::AppConfig;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_CLIENT_NAME: &str = "auto-claude-code-rs";
const MCP_CLIENT_VERSION: &str = "1.0.0";

#[derive(Clone)]
pub struct McpClient {
    http: reqwest::Client,
    base_url: String,
    session_id: Arc<Mutex<Option<String>>>,
    init_lock: Arc<Mutex<()>>,
}

impl McpClient {
    pub fn new(config: &AppConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.mcp.timeout))
            .connect_timeout(Duration::from_secs(config.mcp.connect_timeout))
            .build()
            .context("failed to build mcp reqwest client")?;
        Ok(Self {
            http,
            base_url: config.mcp.base_url.trim().trim_end_matches('/').to_string(),
            session_id: Arc::new(Mutex::new(None)),
            init_lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn list_tool_schemas(&self) -> Result<Vec<Value>> {
        self.ensure_initialized().await?;
        let response = self.send_request("tools/list", json!({})).await?;
        if let Some(error) = response.get("error") {
            bail!("mcp tools/list error: {error}");
        }

        let tools = response
            .get("result")
            .and_then(|value| value.get("tools"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(tools
            .into_iter()
            .filter_map(|tool| build_tool_schema(&tool))
            .collect())
    }

    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> String {
        match self.call_tool_inner(tool_name, arguments).await {
            Ok(output) => output,
            Err(error) => format!("[MCP Error] {error}"),
        }
    }

    async fn call_tool_inner(&self, tool_name: &str, arguments: Value) -> Result<String> {
        self.ensure_initialized().await?;
        let actual_name = tool_name.strip_prefix("mcp_").unwrap_or(tool_name);
        let response = self
            .send_request(
                "tools/call",
                json!({
                    "name": actual_name,
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

fn build_tool_schema(tool: &Value) -> Option<Value> {
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

    Some(json!({
        "type": "function",
        "function": {
            "name": format!("mcp_{name}"),
            "description": format!("[MCP] {description}"),
            "parameters": parameters
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{McpClient, build_tool_schema, parse_response_body};
    use crate::config::model::AppConfig;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::post;
    use serde_json::json;
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
    fn converts_mcp_tools_to_openai_function_schema() {
        let schema = build_tool_schema(&json!({
            "name": "read_file",
            "description": "Read a file via MCP",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }
        }))
        .unwrap();

        assert_eq!(schema["function"]["name"], json!("mcp_read_file"));
        assert_eq!(
            schema["function"]["parameters"]["required"],
            json!(["path"])
        );
    }

    #[tokio::test]
    async fn degrades_gracefully_when_mcp_server_fails() {
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
        config.mcp.connect_timeout = 1;
        config.mcp.timeout = 1;
        let client = McpClient::new(&config).unwrap();

        let list_err = client.list_tool_schemas().await.unwrap_err().to_string();
        assert!(list_err.contains("500 Internal Server Error"));

        let tool_output = client
            .call_tool("mcp_read_file", json!({"path": "foo"}))
            .await;
        assert!(tool_output.starts_with("[MCP Error]"));
        assert!(tool_output.contains("500 Internal Server Error"));

        server.abort();
    }
}
