use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};

use crate::domain::hitl::models::{HitlDecisionRequest, HitlDecisionResolution};

use crate::domain::memory::models::UserMemorySnapshot;
use crate::domain::session::background::{BackgroundManager, BackgroundNotification};
use crate::domain::session::message_bus::MessageBus;
use crate::domain::session::teammate::TeammateManager;
use crate::domain::session::todo::{TodoItem, TodoManager};
use crate::domain::tasks::service::TaskService;
use crate::infra::llm::client::LlmClient;

const DEFAULT_MAX_SESSIONS: usize = 100;
const STEERING_INBOX_NAME: &str = "lead-steering";

#[derive(Clone)]
pub struct SessionContext {
    #[allow(dead_code)]
    pub session_id: String,
    session_dir: PathBuf,
    pub todo: TodoManager,
    pub bg: BackgroundManager,
    pub bus: MessageBus,
    pub team: TeammateManager,
    shutdown_requests: Arc<Mutex<HashMap<String, Value>>>,
    plan_requests: Arc<Mutex<HashMap<String, Value>>>,
    request_counter: Arc<AtomicU64>,
    active_run_generation: Arc<Mutex<Option<u64>>>,
    run_generation_counter: Arc<AtomicU64>,
    prompt_memory_snapshots: Arc<Mutex<HashMap<String, UserMemorySnapshot>>>,
    hitl_waiters: Arc<Mutex<HashMap<String, oneshot::Sender<HitlDecisionResolution>>>>,
    hitl_pending: Arc<Mutex<HashMap<String, HitlDecisionRequest>>>,
}

impl SessionContext {
    fn new(session_id: &str, repo_root: &PathBuf) -> Result<Self> {
        let session_dir = repo_root.join(".sessions").join(session_id);
        fs::create_dir_all(&session_dir)
            .with_context(|| format!("failed to create {}", session_dir.display()))?;
        let team_dir = session_dir.join("team");
        let bus = MessageBus::new(team_dir.clone())?;
        bus.ensure_inbox(STEERING_INBOX_NAME)?;
        Ok(Self {
            session_id: session_id.to_string(),
            session_dir: session_dir.clone(),
            todo: TodoManager::default(),
            bg: BackgroundManager::new(repo_root.clone()),
            team: TeammateManager::new(repo_root.clone(), team_dir, bus.clone())?,
            bus,
            shutdown_requests: Arc::new(Mutex::new(HashMap::new())),
            plan_requests: Arc::new(Mutex::new(HashMap::new())),
            request_counter: Arc::new(AtomicU64::new(1)),
            active_run_generation: Arc::new(Mutex::new(None)),
            run_generation_counter: Arc::new(AtomicU64::new(1)),
            prompt_memory_snapshots: Arc::new(Mutex::new(HashMap::new())),
            hitl_waiters: Arc::new(Mutex::new(HashMap::new())),
            hitl_pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn register_hitl_request(
        &self,
        request: HitlDecisionRequest,
    ) -> oneshot::Receiver<HitlDecisionResolution> {
        let approval_id = request.approval_id.clone();
        let (tx, rx) = oneshot::channel();
        self.hitl_pending
            .lock()
            .expect("hitl pending lock poisoned")
            .insert(approval_id.clone(), request);
        self.hitl_waiters
            .lock()
            .expect("hitl waiters lock poisoned")
            .insert(approval_id, tx);
        rx
    }

    pub fn list_pending_hitl(&self) -> Vec<HitlDecisionRequest> {
        let mut values = self
            .hitl_pending
            .lock()
            .expect("hitl pending lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|left, right| left.created_at_ms.cmp(&right.created_at_ms));
        values
    }

    pub fn resolve_hitl(&self, resolution: HitlDecisionResolution) -> bool {
        self.hitl_pending
            .lock()
            .expect("hitl pending lock poisoned")
            .remove(&resolution.approval_id);
        let sender = self
            .hitl_waiters
            .lock()
            .expect("hitl waiters lock poisoned")
            .remove(&resolution.approval_id);
        if let Some(sender) = sender {
            let _ = sender.send(resolution);
            true
        } else {
            false
        }
    }

    pub async fn wait_hitl_resolution(
        &self,
        approval_id: &str,
        rx: oneshot::Receiver<HitlDecisionResolution>,
        timeout_seconds: u64,
    ) -> HitlDecisionResolution {
        match timeout(Duration::from_secs(timeout_seconds.max(1)), rx).await {
            Ok(Ok(resolution)) => resolution,
            Ok(Err(_)) | Err(_) => {
                self.hitl_pending
                    .lock()
                    .expect("hitl pending lock poisoned")
                    .remove(approval_id);
                self.hitl_waiters
                    .lock()
                    .expect("hitl waiters lock poisoned")
                    .remove(approval_id);
                HitlDecisionResolution::expired(approval_id.to_string())
            }
        }
    }

    pub fn update_todos(&self, items: Vec<TodoItem>) -> Result<String> {
        self.todo.update(items)
    }

    pub fn run_background(&self, command: &str, timeout_secs: u64) -> String {
        self.bg.run(command, timeout_secs)
    }

    pub fn check_background(&self, task_id: Option<&str>) -> String {
        self.bg.check(task_id)
    }

    pub fn send_message(
        &self,
        to: &str,
        content: &str,
        msg_type: &str,
        extra: Option<Map<String, Value>>,
    ) -> Result<String> {
        self.bus.send("lead", to, content, msg_type, extra)
    }

    pub fn read_inbox(&self) -> Result<Vec<Value>> {
        let items = self.bus.read_inbox("lead")?;
        if !items.is_empty() {
            let mut plan_requests = self
                .plan_requests
                .lock()
                .expect("plan requests lock poisoned");
            for item in &items {
                if item.get("type").and_then(Value::as_str) == Some("plan_request")
                    && let Some(request_id) = item
                        .get("request_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                {
                    let from = item
                        .get("from")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let content = item
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    plan_requests.insert(
                        request_id.to_string(),
                        json!({
                            "from": from,
                            "status": "pending",
                            "content": content,
                        }),
                    );
                }
            }
        }
        Ok(items)
    }

    pub fn begin_agent_run(&self) -> u64 {
        let generation = self.run_generation_counter.fetch_add(1, Ordering::SeqCst);
        *self
            .active_run_generation
            .lock()
            .expect("active run generation lock poisoned") = Some(generation);
        generation
    }

    pub fn end_agent_run(&self, generation: u64) {
        let mut active_generation = self
            .active_run_generation
            .lock()
            .expect("active run generation lock poisoned");
        if *active_generation == Some(generation) {
            *active_generation = None;
        }
    }

    pub fn active_run_generation(&self) -> Option<u64> {
        *self
            .active_run_generation
            .lock()
            .expect("active run generation lock poisoned")
    }

    pub fn push_steering_message(&self, content: &str, generation: u64) -> Result<String> {
        self.bus.send(
            "user",
            STEERING_INBOX_NAME,
            content,
            "steering",
            Some(Map::from_iter([(
                "generation".to_string(),
                json!(generation),
            )])),
        )
    }

    pub fn drain_steering_messages(&self, generation: u64) -> Result<Vec<Value>> {
        let items = self.bus.read_inbox(STEERING_INBOX_NAME)?;
        let mut matched = Vec::new();
        let mut dropped = 0usize;
        for item in items {
            if item.get("generation").and_then(Value::as_u64) == Some(generation) {
                matched.push(item);
            } else {
                dropped += 1;
            }
        }
        if dropped > 0 {
            tracing::info!(
                session_id = self.session_id,
                generation,
                dropped,
                "dropped stale steering messages for inactive run generation"
            );
        }
        Ok(matched)
    }

    pub fn broadcast(&self, content: &str) -> Result<String> {
        self.bus.broadcast("lead", content)
    }

    pub async fn spawn_teammate(
        &self,
        name: &str,
        role: &str,
        prompt: &str,
        task_service: &TaskService,
        llm_client: &LlmClient,
        model_id: &str,
    ) -> Result<String> {
        self.team
            .spawn(
                name,
                role,
                prompt,
                task_service.clone(),
                llm_client.clone(),
                model_id.to_string(),
            )
            .await
    }

    pub fn list_teammates(&self) -> String {
        self.team.list_all()
    }

    pub fn handle_shutdown_request(&self, teammate: &str) -> Result<String> {
        let response = self.team.handle_shutdown_request(teammate)?;
        let request_id = response
            .split_whitespace()
            .nth(2)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{:08x}",
                    self.request_counter.fetch_add(1, Ordering::SeqCst)
                )
            });
        self.shutdown_requests
            .lock()
            .expect("shutdown requests lock poisoned")
            .insert(
                request_id,
                json!({
                    "target": teammate,
                    "status": "pending",
                }),
            );
        Ok(response)
    }

    pub fn handle_plan_approval(
        &self,
        request_id: &str,
        approve: bool,
        feedback: &str,
    ) -> Result<String> {
        let mut requests = self
            .plan_requests
            .lock()
            .expect("plan requests lock poisoned");
        let Some(request) = requests.get_mut(request_id) else {
            return Ok(format!("Error: Unknown plan request_id '{request_id}'"));
        };

        let from = request
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status = if approve { "approved" } else { "rejected" };
        request["status"] = json!(status);
        drop(requests);

        self.bus.send(
            "lead",
            &from,
            feedback,
            "plan_approval_response",
            Some(Map::from_iter([
                ("request_id".to_string(), json!(request_id)),
                ("approve".to_string(), json!(approve)),
                ("feedback".to_string(), json!(feedback)),
            ])),
        )?;
        Ok(format!("Plan {status} for '{from}'"))
    }

    pub fn drain_background_notifications(&self) -> Vec<BackgroundNotification> {
        self.bg.drain()
    }

    pub fn get_cached_snapshot(&self, user_id: &str) -> Option<UserMemorySnapshot> {
        self.prompt_memory_snapshots
            .lock()
            .expect("session snapshot lock poisoned")
            .get(user_id)
            .cloned()
    }

    pub fn put_cached_snapshot(&self, snapshot: UserMemorySnapshot) {
        self.prompt_memory_snapshots
            .lock()
            .expect("session snapshot lock poisoned")
            .insert(snapshot.user_id.clone(), snapshot);
    }

    pub fn clear_cached_snapshot(&self, user_id: &str) -> bool {
        self.prompt_memory_snapshots
            .lock()
            .expect("session snapshot lock poisoned")
            .remove(user_id)
            .is_some()
    }

    fn teardown(&self) {
        self.team.shutdown_all();
        let _ = fs::remove_dir_all(&self.session_dir);
    }
}

#[derive(Clone)]
pub struct SessionService {
    repo_root: PathBuf,
    max_sessions: usize,
    inner: Arc<Mutex<SessionStore>>,
}

#[derive(Default)]
struct SessionStore {
    order: VecDeque<String>,
    sessions: HashMap<String, Arc<SessionContext>>,
}

impl SessionService {
    pub fn new(repo_root: PathBuf) -> Self {
        Self::with_max_sessions(repo_root, DEFAULT_MAX_SESSIONS)
    }

    pub fn with_max_sessions(repo_root: PathBuf, max_sessions: usize) -> Self {
        Self {
            repo_root,
            max_sessions: max_sessions.max(1),
            inner: Arc::new(Mutex::new(SessionStore::default())),
        }
    }

    pub fn touch(&self, session_id: &str) {
        let _ = self.get_or_create(session_id);
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<SessionContext>> {
        let mut inner = self.inner.lock().expect("session store lock poisoned");
        let session = inner.sessions.get(session_id).cloned();
        if session.is_some() {
            touch_order(&mut inner.order, session_id);
        }
        session
    }

    pub fn get_or_create(&self, session_id: &str) -> Result<Arc<SessionContext>> {
        let session_id = session_id.trim();
        let mut evicted = Vec::new();
        let session = {
            let mut inner = self.inner.lock().expect("session store lock poisoned");
            if let Some(existing) = inner.sessions.get(session_id).cloned() {
                touch_order(&mut inner.order, session_id);
                existing
            } else {
                while inner.sessions.len() >= self.max_sessions {
                    if let Some(oldest) = inner.order.pop_front() {
                        if let Some(context) = inner.sessions.remove(&oldest) {
                            evicted.push(context);
                        }
                    } else {
                        break;
                    }
                }
                let context = Arc::new(SessionContext::new(session_id, &self.repo_root)?);
                inner
                    .sessions
                    .insert(session_id.to_string(), context.clone());
                inner.order.push_back(session_id.to_string());
                context
            }
        };

        for context in evicted {
            context.teardown();
        }

        Ok(session)
    }

    pub fn delete(&self, session_id: &str) {
        let removed = {
            let mut inner = self.inner.lock().expect("session store lock poisoned");
            touch_remove(&mut inner.order, session_id);
            inner.sessions.remove(session_id)
        };
        if let Some(context) = removed {
            context.teardown();
        }
    }

    pub fn clear_cached_snapshots(&self, user_id: &str) -> usize {
        let sessions = {
            let inner = self.inner.lock().expect("session store lock poisoned");
            inner.sessions.values().cloned().collect::<Vec<_>>()
        };
        sessions
            .into_iter()
            .filter(|context| context.clear_cached_snapshot(user_id))
            .count()
    }

    pub fn session_tool_schemas() -> Vec<Value> {
        vec![
            json!({
                "type": "function",
                "function": {
                    "name": "TodoWrite",
                    "description": "Update the in-session todo checklist. Max 20 items, only one in_progress at a time.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "items": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "content": { "type": "string" },
                                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                                        "activeForm": { "type": "string" }
                                    },
                                    "required": ["content", "status", "activeForm"]
                                }
                            }
                        },
                        "required": ["items"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "background_run",
                    "description": "Run a shell command in the background. Returns a task ID immediately.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string" },
                            "timeout": { "type": "integer", "description": "Timeout in seconds (default 120)" }
                        },
                        "required": ["command"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "check_background",
                    "description": "Check status of a background task, or list all background tasks.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task_id": { "type": "string" }
                        }
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "spawn_teammate",
                    "description": "Spawn an autonomous teammate thread to work on a task.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "role": { "type": "string" },
                            "prompt": { "type": "string" }
                        },
                        "required": ["name", "role", "prompt"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "list_teammates",
                    "description": "List all teammates and their current status.",
                    "parameters": { "type": "object", "properties": {} }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "send_message",
                    "description": "Send a message to a teammate or session inbox.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "to": { "type": "string" },
                            "content": { "type": "string" },
                            "msg_type": { "type": "string", "enum": ["message", "broadcast", "shutdown_request"] }
                        },
                        "required": ["to", "content"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "read_inbox",
                    "description": "Read and clear messages from the lead agent's inbox.",
                    "parameters": { "type": "object", "properties": {} }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "broadcast",
                    "description": "Broadcast a message to all known inboxes in the current session.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" }
                        },
                        "required": ["content"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "claim_task",
                    "description": "Claim a pending task from the board for the lead agent.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task_id": { "type": "integer" }
                        },
                        "required": ["task_id"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "shutdown_request",
                    "description": "Request a teammate to shut down gracefully.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "teammate": { "type": "string" }
                        },
                        "required": ["teammate"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "plan_approval",
                    "description": "Approve or reject a plan submitted by a teammate.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "request_id": { "type": "string" },
                            "approve": { "type": "boolean" },
                            "feedback": { "type": "string" }
                        },
                        "required": ["request_id", "approve"]
                    }
                }
            }),
        ]
    }

    pub async fn dispatch_session_tool(
        &self,
        context: &SessionContext,
        name: &str,
        arguments: &Value,
        task_service: &TaskService,
        llm_client: &LlmClient,
        model_id: &str,
    ) -> Option<String> {
        match name {
            "TodoWrite" => Some(format_result(
                parse_todos(arguments).and_then(|items| context.update_todos(items)),
            )),
            "background_run" => Some(context.run_background(
                required_string(arguments, "command", "background_run").unwrap_or_default(),
                optional_timeout(arguments),
            )),
            "check_background" => {
                Some(context.check_background(optional_string(arguments, "task_id")))
            }
            "spawn_teammate" => Some(
                match (
                    required_string(arguments, "name", "spawn_teammate"),
                    required_string(arguments, "role", "spawn_teammate"),
                    required_string(arguments, "prompt", "spawn_teammate"),
                ) {
                    (Ok(name), Ok(role), Ok(prompt)) => context
                        .spawn_teammate(name, role, prompt, task_service, llm_client, model_id)
                        .await
                        .unwrap_or_else(|error| format!("Error: {error}")),
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        format!("Error: {error}")
                    }
                },
            ),
            "list_teammates" => Some(context.list_teammates()),
            "send_message" => Some(format_result(
                required_string(arguments, "to", "send_message").and_then(|to| {
                    required_string(arguments, "content", "send_message").and_then(|content| {
                        context.send_message(
                            to,
                            content,
                            optional_string(arguments, "msg_type").unwrap_or("message"),
                            None,
                        )
                    })
                }),
            )),
            "read_inbox" => Some(format_result(context.read_inbox().and_then(|items| {
                serde_json::to_string_pretty(&items).context("failed to encode inbox")
            }))),
            "broadcast" => Some(format_result(
                required_string(arguments, "content", "broadcast")
                    .and_then(|content| context.broadcast(content)),
            )),
            "claim_task" => Some(format_result(
                required_u64(arguments, "task_id", "claim_task")
                    .and_then(|task_id| task_service.claim(task_id, "lead")),
            )),
            "shutdown_request" => Some(format_result(
                required_string(arguments, "teammate", "shutdown_request")
                    .and_then(|teammate| context.handle_shutdown_request(teammate)),
            )),
            "plan_approval" => Some(format_result(
                required_string(arguments, "request_id", "plan_approval").and_then(|request_id| {
                    required_bool(arguments, "approve", "plan_approval").and_then(|approve| {
                        context.handle_plan_approval(
                            request_id,
                            approve,
                            optional_string(arguments, "feedback").unwrap_or_default(),
                        )
                    })
                }),
            )),
            _ => None,
        }
    }
}

fn touch_order(order: &mut VecDeque<String>, session_id: &str) {
    touch_remove(order, session_id);
    order.push_back(session_id.to_string());
}

fn touch_remove(order: &mut VecDeque<String>, session_id: &str) {
    if let Some(index) = order.iter().position(|item| item == session_id) {
        order.remove(index);
    }
}

fn parse_todos(arguments: &Value) -> Result<Vec<TodoItem>> {
    let items = arguments.get("items").cloned().unwrap_or_else(|| json!([]));
    serde_json::from_value(items).context("TodoWrite requires an items array")
}

fn required_string<'a>(arguments: &'a Value, key: &str, tool_name: &str) -> Result<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{tool_name} requires a non-empty {key}"))
}

fn optional_string<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_u64(arguments: &Value, key: &str, tool_name: &str) -> Result<u64> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("{tool_name} requires integer {key}"))
}

fn required_bool(arguments: &Value, key: &str, tool_name: &str) -> Result<bool> {
    arguments
        .get(key)
        .and_then(Value::as_bool)
        .with_context(|| format!("{tool_name} requires boolean {key}"))
}

fn optional_timeout(arguments: &Value) -> u64 {
    arguments
        .get("timeout")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(120)
}

fn format_result(result: Result<String>) -> String {
    result.unwrap_or_else(|error| format!("Error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::SessionService;
    use serde_json::json;

    #[test]
    fn isolates_sessions_and_eviction() {
        let repo_root =
            std::env::temp_dir().join(format!("auto-claude-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();

        let service = SessionService::with_max_sessions(repo_root.clone(), 1);
        let a = service.get_or_create("a").unwrap();
        a.update_todos(vec![crate::domain::session::todo::TodoItem {
            content: "Task A".to_string(),
            status: "pending".to_string(),
            active_form: "Doing A".to_string(),
        }])
        .unwrap();
        assert!(a.todo.has_open_items());

        let b = service.get_or_create("b").unwrap();
        assert!(b.todo.list_items().is_empty());
        assert!(service.get("a").is_none());

        service.delete("b");
        assert!(service.get("b").is_none());

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn plan_requests_can_be_approved_after_inbox_read() {
        let repo_root = std::env::temp_dir().join(format!(
            "auto-claude-sessions-plan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();

        let service = SessionService::new(repo_root.clone());
        let ctx = service.get_or_create("demo").unwrap();
        ctx.bus
            .send(
                "alice",
                "lead",
                "Please review this plan",
                "plan_request",
                Some(serde_json::Map::from_iter([(
                    "request_id".to_string(),
                    json!("req-1"),
                )])),
            )
            .unwrap();

        let inbox = ctx.read_inbox().unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0]["type"], json!("plan_request"));

        let result = ctx
            .handle_plan_approval("req-1", true, "looks good")
            .unwrap();
        assert_eq!(result, "Plan approved for 'alice'");

        let alice_inbox = ctx.bus.read_inbox("alice").unwrap();
        assert_eq!(alice_inbox.len(), 1);
        assert_eq!(alice_inbox[0]["type"], json!("plan_approval_response"));
        assert_eq!(alice_inbox[0]["request_id"], json!("req-1"));
        assert_eq!(alice_inbox[0]["approve"], json!(true));
        assert_eq!(alice_inbox[0]["feedback"], json!("looks good"));

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn steering_messages_use_dedicated_inbox() {
        let repo_root = std::env::temp_dir().join(format!(
            "auto-claude-sessions-steering-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();

        let service = SessionService::new(repo_root.clone());
        let ctx = service.get_or_create("demo").unwrap();
        let generation = ctx.begin_agent_run();
        ctx.push_steering_message("Stop after the first tool", generation)
            .unwrap();

        let steering = ctx.drain_steering_messages(generation).unwrap();
        assert_eq!(steering.len(), 1);
        assert_eq!(steering[0]["type"], json!("steering"));
        assert_eq!(steering[0]["content"], json!("Stop after the first tool"));
        assert!(ctx.read_inbox().unwrap().is_empty());
        ctx.end_agent_run(generation);

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn stale_steering_messages_do_not_leak_into_next_run() {
        let repo_root = std::env::temp_dir().join(format!(
            "auto-claude-sessions-steering-stale-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&repo_root);
        std::fs::create_dir_all(&repo_root).unwrap();

        let service = SessionService::new(repo_root.clone());
        let ctx = service.get_or_create("demo").unwrap();
        let first_generation = ctx.begin_agent_run();
        ctx.push_steering_message("Only for first run", first_generation)
            .unwrap();
        ctx.end_agent_run(first_generation);

        let second_generation = ctx.begin_agent_run();
        let steering = ctx.drain_steering_messages(second_generation).unwrap();
        assert!(steering.is_empty());
        ctx.end_agent_run(second_generation);

        let _ = std::fs::remove_dir_all(&repo_root);
    }
}
