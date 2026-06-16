pub mod dto;
pub mod errors;
pub mod routes;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::app_state::SharedState;

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .merge(routes::health::router())
        .nest("/agent", routes::agent_router())
        .nest_service("/static", ServeDir::new(state.repo_root.join("static")))
        .route("/", axum::routing::get(routes::frontend::index))
        .fallback(routes::frontend::fallback)
        .layer(TraceLayer::new_for_http())
        .layer(build_cors_layer(&state))
        .with_state(state)
}

fn build_cors_layer(state: &SharedState) -> CorsLayer {
    let cors = &state.config.server.cors;
    let allow_all_origins = cors.allow_origins.iter().any(|item| item == "*");
    let allow_all_methods = cors.allow_methods.iter().any(|item| item == "*");
    let allow_all_headers = cors.allow_headers.iter().any(|item| item == "*");

    let mut layer = CorsLayer::new();

    layer = if allow_all_origins {
        layer.allow_origin(Any)
    } else {
        let origins = cors
            .allow_origins
            .iter()
            .filter_map(|value| HeaderValue::from_str(value).ok())
            .collect::<Vec<_>>();
        layer.allow_origin(origins)
    };

    layer = if allow_all_methods {
        layer.allow_methods(Any)
    } else {
        let methods = cors
            .allow_methods
            .iter()
            .filter_map(|value| value.parse::<Method>().ok())
            .collect::<Vec<_>>();
        layer.allow_methods(methods)
    };

    layer = if allow_all_headers {
        layer.allow_headers(Any)
    } else {
        let headers = cors
            .allow_headers
            .iter()
            .filter_map(|value| value.parse::<HeaderName>().ok())
            .collect::<Vec<_>>();
        layer.allow_headers(headers)
    };

    if cors.allow_credentials && !allow_all_origins {
        layer = layer.allow_credentials(true);
    }

    layer
}

#[cfg(test)]
mod tests {
    use super::build_router;
    use crate::app_state::{AppState, SharedState};
    use crate::config::model::AppConfig;
    use axum::Router;
    use axum::extract::Json as AxumJson;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::routing::{get, post};
    use reqwest::multipart::{Form, Part};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio::time::{Duration, sleep};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-api-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(root.join("static")).expect("create static dir");
            Self { root }
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct TestServer {
        base_url: String,
        handle: JoinHandle<()>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    struct TestHarness {
        _repo: TestRepo,
        _llm_server: TestServer,
        _app_server: TestServer,
        state: SharedState,
        client: reqwest::Client,
        base_url: String,
        _sample_output_path: PathBuf,
    }

    async fn spawn_server(app: Router) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        TestServer {
            base_url: format!("http://{addr}"),
            handle,
        }
    }

    async fn llm_stub(AxumJson(payload): AxumJson<Value>) -> (StatusCode, HeaderMap, String) {
        let mut headers = HeaderMap::new();
        let stream = payload
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let messages = payload
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has_plan_request_inbox = messages.iter().any(|message| {
            message.get("role") == Some(&json!("user"))
                && message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("\"type\": \"plan_request\"")
        });
        let has_user_prompt = |prompt: &str| {
            messages.iter().any(|message| {
                message.get("role") == Some(&json!("user"))
                    && message
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        == prompt
            })
        };
        let tool_names = payload
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| {
                        tool.get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let last_user = messages.iter().rev().find_map(|message| {
            (message.get("role") == Some(&json!("user"))).then(|| {
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
        });
        let last_tool = messages.iter().rev().find_map(|message| {
            (message.get("role") == Some(&json!("tool"))).then(|| {
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
        });
        let system = payload
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find_map(|message| {
                    (message.get("role") == Some(&json!("system"))).then(|| {
                        message
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                })
            })
            .unwrap_or_default();
        let reply = if system.contains("alias-profile") {
            "saw alias-profile"
        } else {
            "stub reply"
        };

        if stream {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            return (
                StatusCode::OK,
                headers,
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"stub stream\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_string(),
            );
        }

        if has_user_prompt("spawn alice") && last_tool.is_none() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "spawn-1",
                                        "type": "function",
                                        "function": {
                                            "name": "spawn_teammate",
                                            "arguments": "{\"name\":\"alice\",\"role\":\"researcher\",\"prompt\":\"inspect repo\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if has_user_prompt("list team") && last_tool.is_none() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "list-1",
                                        "type": "function",
                                        "function": {
                                            "name": "list_teammates",
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
            );
        }

        if has_user_prompt("shutdown alice") && last_tool.is_none() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "shutdown-1",
                                        "type": "function",
                                        "function": {
                                            "name": "shutdown_request",
                                            "arguments": "{\"teammate\":\"alice\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if has_user_prompt("approve plan") && has_plan_request_inbox && last_tool.is_none() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "approve-1",
                                        "type": "function",
                                        "function": {
                                            "name": "plan_approval",
                                            "arguments": "{\"request_id\":\"req-42\",\"approve\":true,\"feedback\":\"approved by lead\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if last_user.as_deref() == Some("delegate please")
            && tool_names.iter().any(|name| *name == "task")
            && last_tool.is_none()
        {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "task-1",
                                        "type": "function",
                                        "function": {
                                            "name": "task",
                                            "arguments": "{\"prompt\":\"Read README.md and summarize\",\"agent_type\":\"Explore\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if last_user.as_deref() == Some("Read README.md and summarize")
            && tool_names == vec!["read_file"]
            && last_tool.is_none()
        {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": null,
                                "tool_calls": [
                                    {
                                        "id": "read-1",
                                        "type": "function",
                                        "function": {
                                            "name": "read_file",
                                            "arguments": "{\"path\":\"README.md\"}"
                                        }
                                    }
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if last_user.as_deref() == Some("Read README.md and summarize") && last_tool.is_some() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": "subagent summary",
                                "tool_calls": []
                            },
                            "finish_reason": "stop"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if last_user.as_deref() == Some("delegate please") && last_tool.is_some() {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": format!("delegated: {}", last_tool.unwrap_or_default()),
                                "tool_calls": []
                            },
                            "finish_reason": "stop"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if (has_user_prompt("spawn alice")
            || has_user_prompt("list team")
            || has_user_prompt("shutdown alice")
            || has_user_prompt("approve plan"))
            && last_tool.is_some()
        {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": last_tool.unwrap_or_default(),
                                "tool_calls": []
                            },
                            "finish_reason": "stop"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if messages.iter().any(|message| {
            message.get("role") == Some(&json!("user"))
                && message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("<steering>")
        }) {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": "steering applied",
                                "tool_calls": []
                            },
                            "finish_reason": "stop"
                        }
                    ]
                })
                .to_string(),
            );
        }

        if last_user.as_deref() == Some("slow steering run")
            && !messages.iter().any(|message| {
                message.get("role") == Some(&json!("user"))
                    && message
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .contains("<steering>")
            })
        {
            sleep(Duration::from_millis(150)).await;
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            return (
                StatusCode::OK,
                headers,
                json!({
                    "choices": [
                        {
                            "message": {
                                "content": "",
                                "tool_calls": []
                            },
                            "finish_reason": "stop"
                        }
                    ]
                })
                .to_string(),
            );
        }

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
                            "content": reply,
                            "tool_calls": []
                        },
                        "finish_reason": "stop"
                    }
                ]
            })
            .to_string(),
        )
    }

    async fn setup_harness() -> TestHarness {
        let repo = TestRepo::new();
        fs::write(repo.root.join("README.md"), "temporary readme for subagent").unwrap();
        let outputs_dir = repo.root.join("outputs");
        fs::create_dir_all(&outputs_dir).unwrap();
        let sample_output_path = outputs_dir.join("sample.md");
        fs::write(&sample_output_path, "# sample output\n").unwrap();
        let llm_server =
            spawn_server(Router::new().route("/v1/chat/completions", post(llm_stub))).await;

        let mut config = AppConfig::default();
        config.agent.base_url = format!("{}/v1", llm_server.base_url);
        config.agent.model_id = "stub-model".to_string();
        config.mcp.base_url.clear();
        config.skills.learning.enabled = false;

        let state = AppState::new(repo.root.clone(), config).unwrap();
        state.task_service.create("Seed task", "").unwrap();
        state
            .event_service
            .emit(
                "worktree.keep",
                Some(json!({"id": 1})),
                Some(json!({"name": "demo"})),
                None,
            )
            .unwrap();

        let memory_dir = repo.root.join(".user_memories").join("agent-alias");
        fs::create_dir_all(&memory_dir).unwrap();
        fs::write(memory_dir.join("USER.md"), "alias-profile").unwrap();
        fs::write(memory_dir.join("MEMORY.md"), "workspace-memory").unwrap();

        let app_server = spawn_server(build_router(state.clone())).await;
        let client = reqwest::Client::new();
        let base_url = app_server.base_url.clone();

        TestHarness {
            _repo: repo,
            _llm_server: llm_server,
            _app_server: app_server,
            state,
            client,
            base_url,
            _sample_output_path: sample_output_path,
        }
    }

    #[tokio::test]
    async fn smoke_tests_cover_current_agent_endpoints() {
        let harness = setup_harness().await;

        let health = harness
            .client
            .get(format!("{}/health", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(health["status"], json!("ok"));
        assert_eq!(health["model"], json!("stub-model"));

        let mcp_preview = harness
            .client
            .get(format!("{}/agent/settings/mcp", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(mcp_preview["servers"], json!([]));

        let tasks = harness
            .client
            .get(format!("{}/agent/tasks", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert!(
            tasks["tasks"]
                .as_str()
                .unwrap_or_default()
                .contains("Seed task")
        );

        let worktrees = harness
            .client
            .get(format!("{}/agent/worktrees", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(worktrees["worktrees"], json!("No worktrees."));

        let events = harness
            .client
            .get(format!("{}/agent/events", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(events["events"][0]["event"], json!("worktree.keep"));

        let memory = harness
            .client
            .get(format!("{}/agent/memory", harness.base_url))
            .query(&[("agent_id", "agent-alias")])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(memory["user_id"], json!("agent-alias"));
        assert_eq!(memory["count"], json!(2));

        let run = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(Form::new().text("message", "hello").text("history", "[]"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(run["reply"], json!("stub reply"));
        assert_eq!(run["history"].as_array().map(Vec::len), Some(2));

        let delegated = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "delegate please")
                    .text("history", "[]"),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(delegated["reply"], json!("delegated: subagent summary"));

        let stream_body = harness
            .client
            .post(format!("{}/agent/stream", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "stream please")
                    .text("history", "[]")
                    .part(
                        "files",
                        Part::text("hello upload")
                            .file_name("notes.txt")
                            .mime_str("text/plain")
                            .unwrap(),
                    ),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(stream_body.contains("event: files_uploaded"));
        assert!(stream_body.contains("notes.txt"));
        assert!(stream_body.contains("event: text"));
        assert!(stream_body.contains("stub stream"));
        assert!(stream_body.contains("event: done"));

        let memory_run = harness
            .client
            .post(format!("{}/agent/memory/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "remember me")
                    .text("history", "[]")
                    .text("agent_id", "agent-alias"),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(memory_run["reply"], json!("saw alias-profile"));

        assert!(harness.state.session_service.get("missing").is_none());
    }

    #[tokio::test]
    async fn smoke_tests_cover_session_teammate_tools() {
        let harness = setup_harness().await;
        let session_id = "team-demo";

        let spawned = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "spawn alice")
                    .text("history", "[]")
                    .text("session_id", session_id),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            spawned["reply"],
            json!("Spawned 'alice' (role: researcher)")
        );

        let listed = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "list team")
                    .text("history", "[]")
                    .text("session_id", session_id),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let listed_text = listed["reply"].as_str().unwrap_or_default();
        assert!(listed_text.contains("Team: default"));
        assert!(listed_text.contains("alice (researcher):"));

        let ctx = harness
            .state
            .session_service
            .get_or_create(session_id)
            .unwrap();
        ctx.bus
            .send(
                "alice",
                "lead",
                "Please review this plan",
                "plan_request",
                Some(serde_json::Map::from_iter([(
                    "request_id".to_string(),
                    json!("req-42"),
                )])),
            )
            .unwrap();

        let approved = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "approve plan")
                    .text("history", "[]")
                    .text("session_id", session_id),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(approved["reply"], json!("Plan approved for 'alice'"));
        let alice_plan_inbox = ctx.bus.read_inbox("alice").unwrap();
        assert!(
            alice_plan_inbox
                .iter()
                .any(|item| item["type"] == json!("plan_approval_response"))
        );

        let shutdown = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "shutdown alice")
                    .text("history", "[]")
                    .text("session_id", session_id),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let shutdown_reply = shutdown["reply"].as_str().unwrap_or_default().to_string();
        assert!(shutdown_reply.starts_with("Shutdown request "));
        assert!(shutdown_reply.ends_with(" sent to 'alice'"));
        let alice_shutdown_inbox = ctx.bus.read_inbox("alice").unwrap();
        assert!(
            alice_shutdown_inbox
                .iter()
                .any(|item| item["type"] == json!("shutdown_request"))
        );

        harness.state.session_service.delete(session_id);
    }

    #[tokio::test]
    async fn smoke_tests_accept_session_steering_messages() {
        let harness = setup_harness().await;
        let session_id = "steering-demo";
        let ctx = harness
            .state
            .session_service
            .get_or_create(session_id)
            .unwrap();
        let generation = ctx.begin_agent_run();

        let steering = harness
            .client
            .post(format!(
                "{}/agent/session/{}/steering",
                harness.base_url, session_id
            ))
            .header("Content-Type", "application/json")
            .body(json!({ "content": "Please stop after the next tool" }).to_string())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(steering["status"], json!("queued"));
        assert_eq!(steering["session_id"], json!(session_id));

        let steering_items = ctx.drain_steering_messages(generation).unwrap();
        assert_eq!(steering_items.len(), 1);
        assert_eq!(steering_items[0]["type"], json!("steering"));
        assert_eq!(
            steering_items[0]["content"],
            json!("Please stop after the next tool")
        );
        ctx.end_agent_run(generation);

        harness.state.session_service.delete(session_id);
    }

    #[tokio::test]
    async fn smoke_tests_accept_form_encoded_session_steering_messages() {
        let harness = setup_harness().await;
        let session_id = "steering-form-demo";
        let ctx = harness
            .state
            .session_service
            .get_or_create(session_id)
            .unwrap();
        let generation = ctx.begin_agent_run();

        let steering = harness
            .client
            .post(format!(
                "{}/agent/session/{}/steering",
                harness.base_url, session_id
            ))
            .form(&[("content", "Please stop after the next tool")])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(steering["status"], json!("queued"));
        assert_eq!(steering["session_id"], json!(session_id));

        let steering_items = ctx.drain_steering_messages(generation).unwrap();
        assert_eq!(steering_items.len(), 1);
        assert_eq!(steering_items[0]["type"], json!("steering"));
        assert_eq!(
            steering_items[0]["content"],
            json!("Please stop after the next tool")
        );
        ctx.end_agent_run(generation);

        harness.state.session_service.delete(session_id);
    }

    #[tokio::test]
    async fn smoke_tests_apply_steering_to_active_run_without_leaking_to_next_run() {
        let harness = setup_harness().await;
        let session_id = "steering-e2e";
        let ctx = harness
            .state
            .session_service
            .get_or_create(session_id)
            .unwrap();

        let client = harness.client.clone();
        let base_url = harness.base_url.clone();
        let run_handle = tokio::spawn(async move {
            client
                .post(format!("{}/agent/run", base_url))
                .multipart(
                    Form::new()
                        .text("message", "slow steering run")
                        .text("history", "[]")
                        .text("session_id", session_id),
                )
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        });

        for _ in 0..20 {
            if ctx.active_run_generation().is_some() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(
            ctx.active_run_generation().is_some(),
            "expected active run before sending steering"
        );

        let steering = harness
            .client
            .post(format!(
                "{}/agent/session/{}/steering",
                harness.base_url, session_id
            ))
            .header("Content-Type", "application/json")
            .body(json!({ "content": "Please revise" }).to_string())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(steering["status"], json!("queued"));
        assert_eq!(steering["session_id"], json!(session_id));

        let run = run_handle.await.unwrap();
        assert_eq!(run["reply"], json!("steering applied"));

        let next = harness
            .client
            .post(format!("{}/agent/run", harness.base_url))
            .multipart(
                Form::new()
                    .text("message", "hello")
                    .text("history", "[]")
                    .text("session_id", session_id),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(next["reply"], json!("stub reply"));

        harness.state.session_service.delete(session_id);
    }

    #[tokio::test]
    async fn smoke_tests_cover_skills_and_download_endpoints() {
        let harness = setup_harness().await;

        let initial = harness
            .client
            .get(format!("{}/agent/skills?scope=shared", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let initial_count = initial["count"].as_u64().unwrap_or(0);

        let created = harness
            .client
            .post(format!("{}/agent/skills", harness.base_url))
            .header("Content-Type", "application/json")
            .body(
                json!({
                    "name": "demo_skill",
                    "description": "demo description",
                    "tags": "rust,test",
                    "trigger": "demo",
                    "body": "# Demo skill\nUse carefully.",
                    "folder": "demo_skill",
                    "scope": "shared"
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(created["skill"]["name"], json!("demo_skill"));
        assert_eq!(created["scope"], json!("shared"));

        let listed = harness
            .client
            .get(format!("{}/agent/skills?scope=shared", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            listed["count"].as_u64().unwrap_or(0),
            initial_count.saturating_add(1)
        );
        assert!(
            listed["skills"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|item| item["name"] == json!("demo_skill"))
        );

        let updated = harness
            .client
            .put(format!("{}/agent/skills/demo_skill", harness.base_url))
            .header("Content-Type", "application/json")
            .body(
                json!({
                    "name": "demo_skill",
                    "description": "updated description",
                    "tags": "rust,updated",
                    "trigger": "demo",
                    "body": "# Demo skill\nUpdated.",
                    "scope": "shared"
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(
            updated["skill"]["description"],
            json!("updated description")
        );

        let private_created = harness
            .client
            .post(format!("{}/agent/skills", harness.base_url))
            .header("Content-Type", "application/json")
            .body(
                json!({
                    "name": "private_skill",
                    "description": "private desc",
                    "tags": "",
                    "trigger": "",
                    "body": "# Private skill\nUser scoped.",
                    "folder": "private_skill",
                    "scope": "private",
                    "user_id": "agent-alias"
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(private_created["skill"]["scope"], json!("private"));
        assert_eq!(private_created["user_id"], json!("agent-alias"));

        let reloaded = harness
            .client
            .post(format!("{}/agent/skills/reload", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(reloaded["scope"], json!("shared"));

        let remote_skill_server = spawn_server(Router::new().route(
            "/remote/SKILL.md",
            get(|| async {
                "---\nname: remote_skill\ndescription: remote install skill\ntags: hub\n---\n\n# Remote Skill\nUse after hub install.\n"
            }),
        ))
        .await;
        let installed = harness
            .client
            .post(format!("{}/agent/skills/hub/install", harness.base_url))
            .header("Content-Type", "application/json")
            .body(
                json!({
                    "identifier": format!("{}/remote/SKILL.md", remote_skill_server.base_url),
                    "category": "hub-test"
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(installed["skill"]["name"], json!("remote_skill"));
        assert_eq!(installed["installation"]["source"], json!("url"));
        assert_eq!(installed["installation"]["trust_level"], json!("community"));
        assert_eq!(installed["installation"]["scan_verdict"], json!("safe"));

        let hub_installed = harness
            .client
            .get(format!("{}/agent/skills/hub/installed", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(hub_installed["count"], json!(1));
        assert_eq!(
            hub_installed["installations"][0]["name"],
            json!("remote_skill")
        );

        let hub_listed = harness
            .client
            .get(format!("{}/agent/skills?scope=shared", harness.base_url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert!(
            hub_listed["skills"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|item| item["name"] == json!("remote_skill"))
        );
        assert!(
            hub_listed["skills"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .all(|item| !item["path"].as_str().unwrap_or_default().contains("/.hub/"))
        );

        let blocked_delete = harness
            .client
            .delete(format!(
                "{}/agent/skills/remote_skill?scope=shared",
                harness.base_url
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(blocked_delete.status(), StatusCode::BAD_REQUEST);
        let blocked_delete_body = blocked_delete.json::<Value>().await.unwrap();
        assert!(
            blocked_delete_body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("hub 卸载")
        );

        let hub_deleted = harness
            .client
            .delete(format!(
                "{}/agent/skills/hub/remote_skill",
                harness.base_url
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(hub_deleted["deleted"]["name"], json!("remote_skill"));
        assert_eq!(
            hub_deleted["installations"].as_array().map(Vec::len),
            Some(0)
        );

        let deleted = harness
            .client
            .delete(format!(
                "{}/agent/skills/demo_skill?scope=shared",
                harness.base_url
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(deleted["deleted"]["name"], json!("demo_skill"));

        let private_deleted = harness
            .client
            .delete(format!(
                "{}/agent/skills/private_skill?scope=private&user_id=agent-alias",
                harness.base_url
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(private_deleted["deleted"]["name"], json!("private_skill"));

        let download_url = format!("{}/agent/download/outputs/sample.md", harness.base_url);
        let download = harness
            .client
            .get(download_url)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let headers = download.headers().clone();
        let body = download.text().await.unwrap();
        assert_eq!(body, "# sample output\n");
        assert_eq!(
            headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/octet-stream")
        );
        assert!(
            headers
                .get("content-disposition")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .contains("sample.md")
        );
    }
}
