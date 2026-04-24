use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::domain::tasks::service::TaskService;
use crate::infra::fs::tool_ops::dispatch_public_file_tool;
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const TEAMMATE_MAX_TURNS: usize = 50;
const TEAMMATE_MAX_OUTPUT_CHARS: usize = 50_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeamConfig {
    team_name: String,
    members: Vec<TeammateRecord>,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            team_name: "default".to_string(),
            members: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeammateRecord {
    name: String,
    role: String,
    status: String,
}

#[derive(Clone)]
pub struct TeammateManager {
    repo_root: PathBuf,
    bus: super::message_bus::MessageBus,
    config_path: PathBuf,
    config: Arc<Mutex<TeamConfig>>,
    handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    request_counter: Arc<AtomicU64>,
    poll_interval: Duration,
    idle_timeout: Duration,
}

impl TeammateManager {
    pub fn new(
        repo_root: PathBuf,
        team_dir: PathBuf,
        bus: super::message_bus::MessageBus,
    ) -> Result<Self> {
        Self::with_timers(
            repo_root,
            team_dir,
            bus,
            DEFAULT_POLL_INTERVAL,
            DEFAULT_IDLE_TIMEOUT,
        )
    }

    pub fn with_timers(
        repo_root: PathBuf,
        team_dir: PathBuf,
        bus: super::message_bus::MessageBus,
        poll_interval: Duration,
        idle_timeout: Duration,
    ) -> Result<Self> {
        fs::create_dir_all(&team_dir)
            .with_context(|| format!("failed to create {}", team_dir.display()))?;
        let config_path = team_dir.join("config.json");
        let config = if config_path.exists() {
            serde_json::from_str::<TeamConfig>(
                &fs::read_to_string(&config_path)
                    .with_context(|| format!("failed to read {}", config_path.display()))?,
            )
            .with_context(|| format!("failed to parse {}", config_path.display()))?
        } else {
            let default = TeamConfig::default();
            fs::write(
                &config_path,
                serde_json::to_string_pretty(&default)
                    .context("failed to encode default teammate config")?,
            )
            .with_context(|| format!("failed to initialize {}", config_path.display()))?;
            default
        };

        bus.ensure_inbox("lead")?;
        for member in &config.members {
            bus.ensure_inbox(&member.name)?;
        }

        Ok(Self {
            repo_root,
            bus,
            config_path,
            config: Arc::new(Mutex::new(config)),
            handles: Arc::new(Mutex::new(HashMap::new())),
            request_counter: Arc::new(AtomicU64::new(1)),
            poll_interval,
            idle_timeout,
        })
    }

    pub async fn spawn(
        &self,
        name: &str,
        role: &str,
        prompt: &str,
        task_service: TaskService,
        llm_client: LlmClient,
        model_id: String,
    ) -> Result<String> {
        let name = name.trim();
        let role = role.trim();
        let prompt = prompt.trim();
        if name.is_empty() {
            bail!("spawn_teammate requires a non-empty name");
        }
        if role.is_empty() {
            bail!("spawn_teammate requires a non-empty role");
        }
        if prompt.is_empty() {
            bail!("spawn_teammate requires a non-empty prompt");
        }

        {
            let mut config = self.config.lock().expect("teammate config lock poisoned");
            if let Some(member) = config.members.iter_mut().find(|member| member.name == name) {
                if !matches!(member.status.as_str(), "idle" | "shutdown") {
                    return Ok(format!("Error: '{name}' is currently {}", member.status));
                }
                member.role = role.to_string();
                member.status = "working".to_string();
            } else {
                config.members.push(TeammateRecord {
                    name: name.to_string(),
                    role: role.to_string(),
                    status: "working".to_string(),
                });
            }
            self.save_locked(&config)?;
        }

        self.bus.ensure_inbox(name)?;
        if let Some(existing) = self
            .handles
            .lock()
            .expect("teammate handles lock poisoned")
            .remove(name)
        {
            existing.abort();
        }

        let manager = self.clone();
        let name_owned = name.to_string();
        let role_owned = role.to_string();
        let prompt_owned = prompt.to_string();
        let handle = tokio::spawn(async move {
            manager
                .run_loop(
                    name_owned,
                    role_owned,
                    prompt_owned,
                    task_service,
                    llm_client,
                    model_id,
                )
                .await;
        });
        self.handles
            .lock()
            .expect("teammate handles lock poisoned")
            .insert(name.to_string(), handle);

        Ok(format!("Spawned '{name}' (role: {role})"))
    }

    pub fn list_all(&self) -> String {
        let config = self.config.lock().expect("teammate config lock poisoned");
        if config.members.is_empty() {
            return "No teammates.".to_string();
        }

        let handles = self.handles.lock().expect("teammate handles lock poisoned");
        let mut lines = vec![format!("Team: {}", config.team_name)];
        for member in &config.members {
            let alive = handles
                .get(&member.name)
                .map(|handle| !handle.is_finished())
                .unwrap_or(false);
            let thread_status = if alive { "alive" } else { "dead" };
            lines.push(format!(
                "  {} ({}): {} [thread:{}]",
                member.name, member.role, member.status, thread_status
            ));
        }
        lines.join("\n")
    }

    pub fn member_names(&self) -> Vec<String> {
        self.config
            .lock()
            .expect("teammate config lock poisoned")
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect()
    }

    pub fn handle_shutdown_request(&self, teammate: &str) -> Result<String> {
        let teammate = teammate.trim();
        if teammate.is_empty() {
            bail!("shutdown_request requires a non-empty teammate");
        }
        if !self.member_names().iter().any(|member| member == teammate) {
            return Ok(format!("Error: Unknown teammate '{teammate}'"));
        }

        let request_id = format!(
            "{:08x}",
            self.request_counter.fetch_add(1, Ordering::SeqCst)
        );
        self.bus.send(
            "lead",
            teammate,
            "Please shut down.",
            "shutdown_request",
            Some(serde_json::Map::from_iter([(
                "request_id".to_string(),
                json!(request_id),
            )])),
        )?;
        Ok(format!(
            "Shutdown request {request_id} sent to '{teammate}'"
        ))
    }

    pub fn shutdown_all(&self) {
        {
            let mut handles = self.handles.lock().expect("teammate handles lock poisoned");
            for (_, handle) in handles.drain() {
                handle.abort();
            }
        }

        let save_result = {
            let mut config = self.config.lock().expect("teammate config lock poisoned");
            for member in &mut config.members {
                if member.status != "shutdown" {
                    member.status = "shutdown".to_string();
                }
            }
            self.save_locked(&config)
        };
        if let Err(error) = save_result {
            tracing::warn!(?error, "failed to persist teammate shutdown state");
        }
    }

    async fn run_loop(
        &self,
        name: String,
        role: String,
        prompt: String,
        task_service: TaskService,
        llm_client: LlmClient,
        model_id: String,
    ) {
        let team_name = self.team_name();
        let system_prompt = format!(
            "You are '{name}', role: {role}, team: {team_name}, at {}. Use idle when done with current work. You may auto-claim tasks.",
            self.repo_root.display()
        );
        let mut messages = vec![ChatMessage::user(prompt)];

        loop {
            let mut idle_requested = false;
            for _ in 0..TEAMMATE_MAX_TURNS {
                let inbox = match self.bus.read_inbox(&name) {
                    Ok(items) => items,
                    Err(error) => {
                        tracing::warn!(teammate = %name, ?error, "failed to read teammate inbox");
                        let _ = self.set_status(&name, "shutdown");
                        return;
                    }
                };
                for message in inbox {
                    if message.get("type").and_then(Value::as_str) == Some("shutdown_request") {
                        let _ = self.set_status(&name, "shutdown");
                        return;
                    }
                    messages.push(ChatMessage::user(
                        serde_json::to_string(&message).unwrap_or_else(|_| message.to_string()),
                    ));
                }

                let response = match llm_client
                    .chat(&ChatCompletionRequest {
                        model: model_id.clone(),
                        messages: prepend_system_message(&system_prompt, &messages),
                        tools: Some(teammate_tool_schemas()),
                        stream: false,
                        temperature: None,
                        max_tokens: Some(8_000),
                        top_p: None,
                    })
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!(teammate = %name, ?error, "teammate llm call failed");
                        let _ = self.set_status(&name, "shutdown");
                        return;
                    }
                };

                let Some(choice) = response.choices.into_iter().next() else {
                    break;
                };
                let assistant = choice.message;
                let tool_calls = assistant.tool_calls.clone();
                messages.push(assistant.into_chat_message());

                if tool_calls.is_empty() {
                    break;
                }

                for tool_call in &tool_calls {
                    let arguments = serde_json::from_str::<Value>(&tool_call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                    let output = match tool_call.function.name.as_str() {
                        "idle" => {
                            idle_requested = true;
                            "Entering idle phase.".to_string()
                        }
                        "claim_task" => parse_u64_arg(&arguments, "task_id", "claim_task")
                            .and_then(|task_id| task_service.claim(task_id, &name))
                            .unwrap_or_else(|error| format!("Error: {error}")),
                        "send_message" => parse_string_arg(&arguments, "to", "send_message")
                            .and_then(|to| {
                                parse_string_arg(&arguments, "content", "send_message").and_then(
                                    |content| self.bus.send(&name, to, content, "message", None),
                                )
                            })
                            .unwrap_or_else(|error| format!("Error: {error}")),
                        "read_file" | "write_file" | "edit_file" => dispatch_public_file_tool(
                            &self.repo_root,
                            tool_call.function.name.as_str(),
                            &arguments,
                        )
                        .unwrap_or_else(|| format!("Unknown tool: {}", tool_call.function.name)),
                        other => format!("Unknown tool: {other}"),
                    };
                    messages.push(ChatMessage::tool(
                        tool_call.id.clone(),
                        truncate(&output, TEAMMATE_MAX_OUTPUT_CHARS),
                    ));
                }

                if idle_requested {
                    break;
                }
            }

            let _ = self.set_status(&name, "idle");
            let mut resume = false;
            let poll_count = idle_poll_count(self.idle_timeout, self.poll_interval);
            for _ in 0..poll_count {
                tokio::time::sleep(self.poll_interval).await;

                let inbox = match self.bus.read_inbox(&name) {
                    Ok(items) => items,
                    Err(error) => {
                        tracing::warn!(teammate = %name, ?error, "failed to poll teammate inbox");
                        let _ = self.set_status(&name, "shutdown");
                        return;
                    }
                };
                if !inbox.is_empty() {
                    for message in inbox {
                        if message.get("type").and_then(Value::as_str) == Some("shutdown_request") {
                            let _ = self.set_status(&name, "shutdown");
                            return;
                        }
                        messages.push(ChatMessage::user(
                            serde_json::to_string(&message).unwrap_or_else(|_| message.to_string()),
                        ));
                    }
                    resume = true;
                    break;
                }

                let claimable = match task_service.find_claimable_task() {
                    Ok(task) => task,
                    Err(error) => {
                        tracing::warn!(teammate = %name, ?error, "failed to inspect task board");
                        None
                    }
                };
                if let Some(task) = claimable {
                    let _ = task_service.claim(task.id, &name);
                    if messages.len() <= 3 {
                        messages.insert(
                            0,
                            ChatMessage::user(format!(
                                "<identity>You are '{name}', role: {role}, team: {team_name}.</identity>"
                            )),
                        );
                        messages.insert(
                            1,
                            ChatMessage::assistant(
                                Some(format!("I am {name}. Continuing.")),
                                Vec::new(),
                            ),
                        );
                    }
                    messages.push(ChatMessage::user(format!(
                        "<auto-claimed>Task #{}: {}\n{}</auto-claimed>",
                        task.id, task.subject, task.description
                    )));
                    messages.push(ChatMessage::assistant(
                        Some(format!("Claimed task #{}. Working on it.", task.id)),
                        Vec::new(),
                    ));
                    resume = true;
                    break;
                }
            }

            if !resume {
                let _ = self.set_status(&name, "shutdown");
                return;
            }

            let _ = self.set_status(&name, "working");
        }
    }

    fn team_name(&self) -> String {
        self.config
            .lock()
            .expect("teammate config lock poisoned")
            .team_name
            .clone()
    }

    fn set_status(&self, name: &str, status: &str) -> Result<()> {
        let mut config = self.config.lock().expect("teammate config lock poisoned");
        if let Some(member) = config.members.iter_mut().find(|member| member.name == name) {
            member.status = status.to_string();
            self.save_locked(&config)?;
        }
        Ok(())
    }

    fn save_locked(&self, config: &TeamConfig) -> Result<()> {
        fs::write(
            &self.config_path,
            serde_json::to_string_pretty(config).context("failed to encode teammate config")?,
        )
        .with_context(|| format!("failed to write {}", self.config_path.display()))
    }
}

fn prepend_system_message(system_prompt: &str, messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut request_messages = Vec::with_capacity(messages.len() + 1);
    request_messages.push(ChatMessage::system(system_prompt.to_string()));
    request_messages.extend(messages.iter().cloned());
    request_messages
}

fn teammate_tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Edit file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "old_text": { "type": "string" },
                        "new_text": { "type": "string" }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "send_message",
                "description": "Send message.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "to": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["to", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "idle",
                "description": "Signal no more work.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "claim_task",
                "description": "Claim task by ID.",
                "parameters": {
                    "type": "object",
                    "properties": { "task_id": { "type": "integer" } },
                    "required": ["task_id"]
                }
            }
        }),
    ]
}

fn parse_string_arg<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{tool_name} requires a non-empty {key}"))
}

fn parse_u64_arg(arguments: &Value, key: &str, tool_name: &str) -> Result<u64> {
    let raw = arguments
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("{tool_name} requires an integer {key}"))?;
    Ok(raw)
}

fn idle_poll_count(idle_timeout: Duration, poll_interval: Duration) -> usize {
    if idle_timeout.is_zero() {
        return 1;
    }
    let poll_ms = poll_interval.as_millis().max(1);
    let idle_ms = idle_timeout.as_millis().max(1);
    usize::try_from(idle_ms.div_ceil(poll_ms))
        .unwrap_or(1)
        .max(1)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::TeammateManager;
    use crate::config::model::AppConfig;
    use crate::domain::session::message_bus::MessageBus;
    use crate::domain::tasks::service::TaskService;
    use crate::infra::llm::client::LlmClient;
    use axum::Router;
    use axum::extract::Json as AxumJson;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::routing::post;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let unique = format!(
                "{prefix}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct TestServer {
        handle: JoinHandle<()>,
        base_url: String,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn spawn_server(app: Router) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestServer {
            handle,
            base_url: format!("http://{addr}"),
        }
    }

    async fn llm_stub(AxumJson(_payload): AxumJson<Value>) -> (StatusCode, HeaderMap, String) {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        (
            StatusCode::OK,
            headers,
            json!({
                "choices": [
                    {
                        "message": {
                            "content": null,
                            "tool_calls": [
                                {
                                    "id": "idle-1",
                                    "type": "function",
                                    "function": {
                                        "name": "idle",
                                        "arguments": "{}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ]
            })
            .to_string(),
        )
    }

    #[tokio::test]
    async fn spawns_lists_and_persists_teammates() {
        let repo = TestDir::new("auto-claude-team");
        let team_dir = repo.path.join(".sessions/demo/team");
        let bus = MessageBus::new(team_dir.clone()).unwrap();
        let manager = TeammateManager::with_timers(
            repo.path.clone(),
            team_dir.clone(),
            bus,
            Duration::from_millis(10),
            Duration::from_millis(30),
        )
        .unwrap();

        let llm_server =
            spawn_server(Router::new().route("/v1/chat/completions", post(llm_stub))).await;
        let mut config = AppConfig::default();
        config.agent.base_url = format!("{}/v1", llm_server.base_url);
        let llm_client = LlmClient::new(&config).unwrap();
        let tasks = TaskService::new(repo.path.clone()).unwrap();

        let result = manager
            .spawn(
                "alice",
                "researcher",
                "inspect repo",
                tasks,
                llm_client,
                "stub-model".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(result, "Spawned 'alice' (role: researcher)");
        assert!(manager.list_all().contains("alice (researcher): working"));

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(manager.list_all().contains("alice (researcher): shutdown"));

        let reopened = TeammateManager::with_timers(
            repo.path.clone(),
            team_dir,
            crate::domain::session::message_bus::MessageBus::new(
                repo.path.join(".sessions/demo/team"),
            )
            .unwrap(),
            Duration::from_millis(10),
            Duration::from_millis(30),
        )
        .unwrap();
        assert!(reopened.list_all().contains("alice (researcher): shutdown"));
    }

    #[test]
    fn shutdown_request_requires_known_teammate() {
        let repo = TestDir::new("auto-claude-team-shutdown");
        let team_dir = repo.path.join(".sessions/demo/team");
        let bus = MessageBus::new(team_dir.clone()).unwrap();
        let manager = TeammateManager::new(repo.path.clone(), team_dir, bus).unwrap();
        assert_eq!(
            manager.handle_shutdown_request("ghost").unwrap(),
            "Error: Unknown teammate 'ghost'"
        );
    }
}
