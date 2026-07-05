use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{Method, header};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router, body::Bytes};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::dto::chat::{AgentResponse, MemoryAgentResponse, SteeringResponse};
use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::domain::chat::models::{
    AgentPromptOverrides, BuiltinToolOverrides, ChatEvent, ChatMode, ChatRequest, HistoryEntry,
    HitlOverrides, LlmOverrides, McpOverrides, SkillPermissions, SteeringSubmission, UploadedFile,
    is_builtin_file_tool, normalize_builtin_tool_list,
};
use crate::domain::hitl::models::HitlDecisionResolution;
use crate::domain::hitl::policy::HitlDefaultAction;
use crate::domain::hitl::service::write_hitl_resolution;
use crate::domain::settings::{
    SharedFrontendSettings, UserMcpPermissions, UserSkillPermissions,
    load_shared_frontend_settings, save_shared_frontend_settings,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/system-prompt", get(agent_system_prompt))
        .route("/run", post(agent_run))
        .route("/stream", post(agent_stream))
        .route("/memory/run", post(agent_memory_run))
        .route("/memory/stream", post(agent_memory_stream))
        .route(
            "/session/{session_id}/steering",
            post(agent_session_steering).options(agent_session_steering_options),
        )
        .route(
            "/session/{session_id}/approvals",
            get(list_session_approvals),
        )
        .route(
            "/session/{session_id}/approvals/{approval_id}/approve",
            post(approve_session_approval),
        )
        .route(
            "/session/{session_id}/approvals/{approval_id}/reject",
            post(reject_session_approval),
        )
        .route(
            "/session/{session_id}/approvals/{approval_id}/modify",
            post(modify_session_approval),
        )
        .route("/settings/prompts", get(agent_prompt_settings))
        .route("/settings/mcp", get(agent_mcp_settings))
        .route(
            "/settings/shared",
            get(get_shared_frontend_settings).post(save_shared_frontend_settings_route),
        )
}

#[derive(Debug, Deserialize)]
struct SystemPromptQuery {
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpSettingsQuery {
    config_path: Option<String>,
    base_urls: Option<String>,
    disabled_urls: Option<String>,
    lazy_urls: Option<String>,
    user_id: Option<String>,
    allowed_tools: Option<String>,
    denied_tools: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SteeringRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct HitlResolveRequest {
    resolved_by: String,
    note: Option<String>,
    arguments: Option<serde_json::Value>,
}

async fn agent_system_prompt(
    State(state): State<SharedState>,
    Query(query): Query<SystemPromptQuery>,
) -> ApiResult<Json<crate::domain::chat::models::SystemPromptPreview>> {
    let preview = state
        .chat_orchestrator_clone()
        .preview_system_prompts(query.user_id.as_deref())?;
    Ok(Json(preview))
}

async fn agent_prompt_settings(
    State(state): State<SharedState>,
) -> ApiResult<Json<crate::domain::chat::models::AgentPromptSettingsPreview>> {
    Ok(Json(
        state
            .chat_orchestrator_clone()
            .preview_agent_prompt_settings(),
    ))
}

async fn agent_mcp_settings(
    State(state): State<SharedState>,
    Query(query): Query<McpSettingsQuery>,
) -> ApiResult<Json<crate::domain::chat::models::McpSettingsPreview>> {
    let settings = load_shared_frontend_settings(&state.repo_root).await?;
    let user_mcp_permissions =
        resolve_user_mcp_permissions(&settings.mcp_user_permissions, query.user_id.as_deref());
    let allowed_tools = parse_string_list(query.allowed_tools.as_deref(), "allowed_tools")?;
    let denied_tools = parse_string_list(query.denied_tools.as_deref(), "denied_tools")?;
    let overrides = McpOverrides {
        config_path: query.config_path.and_then(non_empty),
        base_urls: parse_mcp_base_urls(query.base_urls.as_deref())?,
        disabled_urls: parse_mcp_base_urls(query.disabled_urls.as_deref())?,
        lazy_urls: parse_mcp_base_urls(query.lazy_urls.as_deref())?,
        allowed_tools: if allowed_tools.is_empty() {
            user_mcp_permissions.allowed_tools
        } else {
            allowed_tools
        },
        denied_tools: if denied_tools.is_empty() {
            user_mcp_permissions.denied_tools
        } else {
            denied_tools
        },
    };
    Ok(Json(
        state
            .chat_orchestrator_clone()
            .preview_mcp_settings(overrides)
            .await?,
    ))
}

async fn get_shared_frontend_settings(
    State(state): State<SharedState>,
) -> ApiResult<Json<SharedFrontendSettings>> {
    Ok(Json(load_shared_frontend_settings(&state.repo_root).await?))
}

async fn save_shared_frontend_settings_route(
    State(state): State<SharedState>,
    Json(payload): Json<SharedFrontendSettings>,
) -> ApiResult<Json<SharedFrontendSettings>> {
    let normalized = payload.normalized();
    save_shared_frontend_settings(&state.repo_root, &normalized).await?;
    Ok(Json(normalized))
}

async fn agent_run(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> ApiResult<Json<AgentResponse>> {
    let request = parse_chat_multipart(state.clone(), multipart).await?;
    if let Some(session_id) = request.session_id.as_deref() {
        state.session_service.touch(session_id);
    }
    let result = state
        .chat_orchestrator_clone()
        .run(request, ChatMode::Stateless)
        .await?;
    Ok(Json(AgentResponse {
        reply: result.reply,
        history: result.history,
        output_files: result.output_files,
        token_usage: result.token_usage,
    }))
}

async fn agent_memory_run(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> ApiResult<Json<MemoryAgentResponse>> {
    let request = parse_chat_multipart(state.clone(), multipart).await?;
    if let Some(session_id) = request.session_id.as_deref() {
        state.session_service.touch(session_id);
    }
    let result = state
        .chat_orchestrator_clone()
        .run(request, ChatMode::Memory)
        .await?;
    Ok(Json(MemoryAgentResponse {
        reply: result.reply,
        history: result.history,
        skills_updated: result.skills_updated,
        output_files: result.output_files,
        token_usage: result.token_usage,
    }))
}

async fn agent_stream(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let request = parse_chat_multipart(state.clone(), multipart).await?;
    if let Some(session_id) = request.session_id.as_deref() {
        state.session_service.touch(session_id);
    }
    build_stream_response(state, request, ChatMode::Stateless).await
}

async fn agent_memory_stream(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let request = parse_chat_multipart(state.clone(), multipart).await?;
    if let Some(session_id) = request.session_id.as_deref() {
        state.session_service.touch(session_id);
    }
    build_stream_response(state, request, ChatMode::Memory).await
}

async fn list_session_approvals(
    Path(session_id): Path<String>,
    State(state): State<SharedState>,
) -> ApiResult<Json<Vec<crate::domain::hitl::models::HitlDecisionRequest>>> {
    let session = state.session_service.get_or_create(&session_id)?;
    Ok(Json(session.list_pending_hitl()))
}

async fn approve_session_approval(
    Path((session_id, approval_id)): Path<(String, String)>,
    State(state): State<SharedState>,
    Json(payload): Json<HitlResolveRequest>,
) -> ApiResult<Json<HitlDecisionResolution>> {
    resolve_session_approval(session_id, approval_id, state, payload, "approve").await
}

async fn reject_session_approval(
    Path((session_id, approval_id)): Path<(String, String)>,
    State(state): State<SharedState>,
    Json(payload): Json<HitlResolveRequest>,
) -> ApiResult<Json<HitlDecisionResolution>> {
    resolve_session_approval(session_id, approval_id, state, payload, "reject").await
}

async fn modify_session_approval(
    Path((session_id, approval_id)): Path<(String, String)>,
    State(state): State<SharedState>,
    Json(payload): Json<HitlResolveRequest>,
) -> ApiResult<Json<HitlDecisionResolution>> {
    resolve_session_approval(session_id, approval_id, state, payload, "modify").await
}

async fn resolve_session_approval(
    session_id: String,
    approval_id: String,
    state: SharedState,
    payload: HitlResolveRequest,
    action: &str,
) -> ApiResult<Json<HitlDecisionResolution>> {
    let resolved_by = payload.resolved_by.trim().to_string();
    if resolved_by.is_empty() {
        return Err(ApiError::bad_request("resolved_by 不能为空"));
    }
    let session = state.session_service.get_or_create(&session_id)?;
    let resolution = match action {
        "approve" => HitlDecisionResolution::approved(approval_id, resolved_by, payload.note),
        "reject" => HitlDecisionResolution::rejected(approval_id, resolved_by, payload.note),
        "modify" => HitlDecisionResolution::modified(
            approval_id,
            resolved_by,
            payload.note,
            payload.arguments.unwrap_or_else(|| serde_json::json!({})),
        ),
        _ => return Err(ApiError::bad_request("unknown HITL action")),
    };
    if !session.resolve_hitl(resolution.clone()) {
        return Err(ApiError::not_found(format!(
            "approval not found or already resolved: {}",
            resolution.approval_id
        )));
    }
    write_hitl_resolution(&state.repo_root, &resolution).await?;
    Ok(Json(resolution))
}

async fn agent_session_steering(
    Path(session_id): Path<String>,
    State(state): State<SharedState>,
    method: Method,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> ApiResult<Json<SteeringResponse>> {
    let payload = parse_steering_request(method, &headers, &body)?;
    let content = payload.content.trim();
    if content.is_empty() {
        return Err(ApiError::bad_request("content 不能为空"));
    }
    let session = state.session_service.get_or_create(&session_id)?;
    let Some(generation) = session.active_run_generation() else {
        return Err(ApiError::bad_request(
            "当前没有正在运行的 agent，无法注入 steering",
        ));
    };
    session.push_steering_message(content, generation)?;
    Ok(Json(
        SteeringSubmission {
            status: "queued".to_string(),
            session_id,
        }
        .into(),
    ))
}

async fn agent_session_steering_options() -> impl IntoResponse {
    axum::http::StatusCode::NO_CONTENT
}

fn parse_steering_request(
    method: Method,
    headers: &axum::http::HeaderMap,
    body: &Bytes,
) -> ApiResult<SteeringRequest> {
    if method != Method::POST {
        return Err(ApiError::bad_request("仅支持 POST"));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if content_type.contains("application/json") {
        return serde_json::from_slice::<SteeringRequest>(body)
            .map_err(|_| ApiError::bad_request("steering 请求体必须是合法 JSON"));
    }

    if content_type.contains("application/x-www-form-urlencoded") || content_type.is_empty() {
        return serde_urlencoded::from_bytes::<SteeringRequest>(body)
            .map_err(|_| ApiError::bad_request("steering 表单参数不合法"));
    }

    Err(ApiError::bad_request(format!(
        "不支持的 steering Content-Type: {}",
        content_type
    )))
}

async fn build_stream_response(
    state: SharedState,
    request: ChatRequest,
    mode: ChatMode,
) -> ApiResult<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>> {
    let (tx, rx) = mpsc::channel::<ChatEvent>(64);
    if !request.files.is_empty() {
        let _ = tx
            .send(ChatEvent::FilesUploaded(request.files.clone()))
            .await;
    }
    tokio::spawn(async move {
        let result = state
            .chat_orchestrator_clone()
            .stream(request, mode, tx.clone())
            .await;
        if let Err(error) = result {
            let _ = tx
                .send(ChatEvent::Error {
                    detail: error.to_string(),
                })
                .await;
        }
    });

    let stream = ReceiverStream::new(rx).map(|event| Ok(chat_event_to_sse(event)));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn parse_chat_multipart(
    state: SharedState,
    mut multipart: Multipart,
) -> ApiResult<ChatRequest> {
    let mut message: Option<String> = None;
    let mut history_json = String::from("[]");
    let mut system: Option<String> = None;
    let mut system_override: Option<String> = None;
    let mut system_append: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut user_id: Option<String> = None;
    let mut agent_id: Option<String> = None;
    let mut model_id: Option<String> = None;
    let mut temperature: Option<f32> = None;
    let mut max_tokens: Option<u32> = None;
    let mut max_iterations: Option<usize> = None;
    let mut top_p: Option<f32> = None;
    let mut memory_maintenance_system: Option<String> = None;
    let mut memory_maintenance_user_template: Option<String> = None;
    let mut skill_learning_system: Option<String> = None;
    let mut skill_learning_user_template: Option<String> = None;
    let mut mcp_config_path: Option<String> = None;
    let mut mcp_base_urls_json: Option<String> = None;
    let mut mcp_disabled_urls_json: Option<String> = None;
    let mut mcp_lazy_urls_json: Option<String> = None;
    let mut mcp_allowed_tools_json: Option<String> = None;
    let mut mcp_denied_tools_json: Option<String> = None;
    let mut skill_allowed_names_json: Option<String> = None;
    let mut skill_denied_names_json: Option<String> = None;
    let mut builtin_tools_enabled: Option<bool> = None;
    let mut builtin_allowed_tools_json: Option<String> = None;
    let mut builtin_denied_tools_json: Option<String> = None;
    let mut hitl_enabled: Option<bool> = None;
    let mut hitl_default_action: Option<HitlDefaultAction> = None;
    let mut hitl_timeout_seconds: Option<u64> = None;
    let mut hitl_rules_json: Option<String> = None;
    let mut files = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(anyhow::Error::from)? {
        let name = field.name().unwrap_or_default().to_string();
        if name == "files" {
            files.push(save_upload(state.clone(), field).await?);
            continue;
        }

        let value = field.text().await.map_err(anyhow::Error::from)?;
        match name.as_str() {
            "message" => message = Some(value),
            "history" => history_json = value,
            "system" => system = non_empty(value),
            "system_override" => system_override = non_empty(value),
            "system_append" => system_append = non_empty(value),
            "session_id" => session_id = non_empty(value),
            "user_id" => user_id = non_empty(value),
            "agent_id" => agent_id = non_empty(value),
            "model_id" => model_id = non_empty(value),
            "temperature" => temperature = parse_optional_number::<f32>(&value, "temperature")?,
            "max_tokens" => max_tokens = parse_optional_number::<u32>(&value, "max_tokens")?,
            "max_iterations" => {
                max_iterations = parse_optional_number::<usize>(&value, "max_iterations")?
            }
            "top_p" => top_p = parse_optional_number::<f32>(&value, "top_p")?,
            "memory_maintenance_system" => memory_maintenance_system = non_empty(value),
            "memory_maintenance_user_template" => {
                memory_maintenance_user_template = non_empty(value)
            }
            "skill_learning_system" => skill_learning_system = non_empty(value),
            "skill_learning_user_template" => skill_learning_user_template = non_empty(value),
            "mcp_config_path" => mcp_config_path = non_empty(value),
            "mcp_base_urls" => mcp_base_urls_json = non_empty(value),
            "mcp_disabled_urls" => mcp_disabled_urls_json = non_empty(value),
            "mcp_lazy_urls" => mcp_lazy_urls_json = non_empty(value),
            "mcp_allowed_tools" => mcp_allowed_tools_json = non_empty(value),
            "mcp_denied_tools" => mcp_denied_tools_json = non_empty(value),
            "skill_allowed_names" => skill_allowed_names_json = non_empty(value),
            "skill_denied_names" => skill_denied_names_json = non_empty(value),
            "builtin_tools_enabled" => {
                builtin_tools_enabled = Some(parse_bool_field(&value, "builtin_tools_enabled")?)
            }
            "builtin_allowed_tools" => builtin_allowed_tools_json = non_empty(value),
            "builtin_denied_tools" => builtin_denied_tools_json = non_empty(value),
            "hitl_enabled" => hitl_enabled = Some(parse_bool_field(&value, "hitl_enabled")?),
            "hitl_default_action" => hitl_default_action = Some(parse_hitl_default_action(&value)?),
            "hitl_timeout_seconds" => {
                hitl_timeout_seconds = Some(
                    parse_optional_number::<u64>(&value, "hitl_timeout_seconds")?.unwrap_or(300),
                )
            }
            "hitl_rules" => hitl_rules_json = non_empty(value),
            _ => {}
        }
    }

    let message = message
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("message 不能为空"))?;
    let history = parse_history(&history_json)?;

    if let Some(value) = temperature {
        if !(0.0..=2.0).contains(&value) {
            return Err(ApiError::bad_request("temperature 必须在 0 到 2 之间"));
        }
    }
    if let Some(value) = top_p {
        if !(0.0..=1.0).contains(&value) || value == 0.0 {
            return Err(ApiError::bad_request("top_p 必须在 0 到 1 之间"));
        }
    }
    if let Some(value) = max_tokens {
        if value == 0 {
            return Err(ApiError::bad_request("max_tokens 必须大于 0"));
        }
    }
    if let Some(value) = max_iterations {
        if value == 0 {
            return Err(ApiError::bad_request("max_iterations 必须大于 0"));
        }
    }
    let mcp_base_urls = parse_mcp_base_urls(mcp_base_urls_json.as_deref())?;
    let mcp_disabled_urls = parse_mcp_base_urls(mcp_disabled_urls_json.as_deref())?;
    let mcp_lazy_urls = parse_mcp_base_urls(mcp_lazy_urls_json.as_deref())?;
    let mut mcp_allowed_tools =
        parse_string_list(mcp_allowed_tools_json.as_deref(), "mcp_allowed_tools")?;
    let mut mcp_denied_tools =
        parse_string_list(mcp_denied_tools_json.as_deref(), "mcp_denied_tools")?;
    let mut skill_allowed_names =
        parse_string_list(skill_allowed_names_json.as_deref(), "skill_allowed_names")?;
    let mut skill_denied_names =
        parse_string_list(skill_denied_names_json.as_deref(), "skill_denied_names")?;
    let resolved_user_id = user_id.or(agent_id);

    if mcp_allowed_tools.is_empty()
        && mcp_denied_tools.is_empty()
        && skill_allowed_names.is_empty()
        && skill_denied_names.is_empty()
    {
        let settings = load_shared_frontend_settings(&state.repo_root).await?;
        let user_mcp_permissions = resolve_user_mcp_permissions(
            &settings.mcp_user_permissions,
            resolved_user_id.as_deref(),
        );
        let user_skill_permissions = resolve_user_skill_permissions(
            &settings.skill_user_permissions,
            resolved_user_id.as_deref(),
        );
        mcp_allowed_tools = user_mcp_permissions.allowed_tools;
        mcp_denied_tools = user_mcp_permissions.denied_tools;
        skill_allowed_names = user_skill_permissions.allowed_skills;
        skill_denied_names = user_skill_permissions.denied_skills;
    }

    let settings = load_shared_frontend_settings(&state.repo_root).await?;
    let hitl_timeout_raw = hitl_timeout_seconds
        .or_else(|| settings.hitl_timeout_seconds.parse::<u64>().ok())
        .unwrap_or(state.config.hitl.timeout_seconds)
        .max(1);
    let hitl_rules = if let Some(raw) = hitl_rules_json.as_deref() {
        parse_hitl_rules(raw)?
    } else if !settings.hitl_rules.is_empty() {
        settings.hitl_rules.clone()
    } else {
        state.config.hitl.rules.clone()
    };
    let hitl_overrides = HitlOverrides {
        enabled: hitl_enabled.unwrap_or(settings.hitl_enabled || state.config.hitl.enabled),
        default_action: hitl_default_action
            .or_else(|| {
                if settings.hitl_enabled {
                    Some(settings.hitl_default_action.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| state.config.hitl.default_action.clone()),
        timeout_seconds: hitl_timeout_raw,
        rules: hitl_rules,
    };
    let builtin_allowed_tools = if let Some(raw) = builtin_allowed_tools_json.as_deref() {
        parse_builtin_tool_list(raw, "builtin_allowed_tools")?
    } else {
        normalize_builtin_tool_list(state.config.builtin_tools.allowed_tools.clone())
    };
    let builtin_denied_tools = if let Some(raw) = builtin_denied_tools_json.as_deref() {
        parse_builtin_tool_list(raw, "builtin_denied_tools")?
    } else {
        normalize_builtin_tool_list(state.config.builtin_tools.denied_tools.clone())
    };
    let builtin_tool_overrides = BuiltinToolOverrides {
        enabled: builtin_tools_enabled.unwrap_or(state.config.builtin_tools.enabled),
        allowed_tools: builtin_allowed_tools,
        denied_tools: builtin_denied_tools,
    };

    Ok(ChatRequest {
        message,
        history,
        system: system.or(system_override.clone()),
        system_override,
        system_append,
        session_id,
        user_id: resolved_user_id,
        files,
        llm_overrides: LlmOverrides {
            model_id,
            temperature,
            max_tokens,
            max_iterations,
            top_p,
        },
        prompt_overrides: AgentPromptOverrides {
            memory_maintenance_system,
            memory_maintenance_user_template,
            skill_learning_system,
            skill_learning_user_template,
        },
        mcp_overrides: McpOverrides {
            config_path: mcp_config_path,
            base_urls: mcp_base_urls,
            disabled_urls: mcp_disabled_urls,
            lazy_urls: mcp_lazy_urls,
            allowed_tools: mcp_allowed_tools,
            denied_tools: mcp_denied_tools,
        },
        builtin_tool_overrides,
        skill_permissions: SkillPermissions {
            allowed_skills: skill_allowed_names,
            denied_skills: skill_denied_names,
        },
        hitl_overrides,
    })
}

async fn save_upload(
    state: SharedState,
    field: axum::extract::multipart::Field<'_>,
) -> ApiResult<UploadedFile> {
    let filename = field
        .file_name()
        .map(|value| value.to_string())
        .ok_or_else(|| ApiError::bad_request("上传文件缺少文件名"))?;
    let content_type = field.content_type().map(|value| value.to_string());
    let bytes = field.bytes().await.map_err(anyhow::Error::from)?;
    let upload_dir = state.repo_root.join("uploads");
    tokio::fs::create_dir_all(&upload_dir)
        .await
        .map_err(anyhow::Error::from)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(anyhow::Error::from)?
        .as_secs();
    let safe_filename = format!(
        "{}_{}_{}",
        timestamp,
        filesafe_fragment(&filename),
        filename
    );
    let saved_path = upload_dir.join(safe_filename);
    tokio::fs::write(&saved_path, &bytes)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(UploadedFile {
        original_name: filename,
        saved_path: saved_path.display().to_string(),
        size: bytes.len(),
        content_type,
    })
}

fn parse_history(raw: &str) -> ApiResult<Vec<HistoryEntry>> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| ApiError::bad_request("history 必须是合法 JSON"))?;
    let Some(items) = value.as_array() else {
        return Err(ApiError::bad_request("history 必须是数组"));
    };

    let mut history = Vec::new();
    for item in items {
        let role = item
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let content = item
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        if role.is_empty() || content.is_empty() {
            continue;
        }
        history.push(HistoryEntry { role, content });
    }
    Ok(history)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_bool_field(value: &str, field: &str) -> ApiResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ApiError::bad_request(format!("{field} 必须是布尔值"))),
    }
}

fn parse_hitl_default_action(value: &str) -> ApiResult<HitlDefaultAction> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(HitlDefaultAction::Auto),
        "require_approval" => Ok(HitlDefaultAction::RequireApproval),
        "reject" => Ok(HitlDefaultAction::Reject),
        _ => Err(ApiError::bad_request("hitl_default_action 不合法")),
    }
}

fn parse_hitl_rules(value: &str) -> ApiResult<Vec<crate::domain::hitl::policy::HitlPolicyRule>> {
    serde_json::from_str::<Vec<crate::domain::hitl::policy::HitlPolicyRule>>(value)
        .map_err(|_| ApiError::bad_request("hitl_rules 必须是合法 JSON 数组"))
}

fn parse_optional_number<T>(raw: &str, label: &str) -> ApiResult<Option<T>>
where
    T: std::str::FromStr,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<T>()
        .map(Some)
        .map_err(|_| ApiError::bad_request(format!("{label} 格式不正确")))
}

fn parse_string_list(raw: Option<&str>, label: &str) -> ApiResult<Vec<String>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };

    let parsed = serde_json::from_str::<Vec<String>>(raw)
        .map_err(|_| ApiError::bad_request(format!("{label} 必须是字符串数组 JSON")))?;

    let mut seen = std::collections::HashSet::new();
    Ok(parsed
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .filter(|item| seen.insert(item.clone()))
        .collect())
}

fn parse_builtin_tool_list(raw: &str, label: &str) -> ApiResult<Vec<String>> {
    let parsed = parse_string_list(Some(raw), label)?;
    let invalid = parsed
        .iter()
        .filter(|item| !is_builtin_file_tool(item))
        .cloned()
        .collect::<Vec<_>>();
    if !invalid.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{label} 包含不支持的内置工具: {}",
            invalid.join(", ")
        )));
    }
    Ok(normalize_builtin_tool_list(parsed))
}

fn parse_mcp_base_urls(raw: Option<&str>) -> ApiResult<Vec<String>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };

    parse_string_list(Some(raw), "mcp_base_urls")
}

fn resolve_user_mcp_permissions(
    rules: &[UserMcpPermissions],
    user_id: Option<&str>,
) -> UserMcpPermissions {
    let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return UserMcpPermissions::default();
    };
    rules
        .iter()
        .find(|rule| rule.user_id.trim() == user_id)
        .cloned()
        .unwrap_or_default()
}

fn resolve_user_skill_permissions(
    rules: &[UserSkillPermissions],
    user_id: Option<&str>,
) -> SkillPermissions {
    let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return SkillPermissions::default();
    };
    rules
        .iter()
        .find(|rule| rule.user_id.trim() == user_id)
        .map(|rule| SkillPermissions {
            allowed_skills: rule.allowed_skills.clone(),
            denied_skills: rule.denied_skills.clone(),
        })
        .unwrap_or_default()
}

fn filesafe_fragment(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn chat_event_to_sse(event: ChatEvent) -> Event {
    match event {
        ChatEvent::Text(text) => sse_event("text", serde_json::json!({ "text": text })),
        ChatEvent::ToolUse { name, arguments } => sse_event(
            "tool_use",
            serde_json::json!({ "name": name, "arguments": arguments }),
        ),
        ChatEvent::ToolResult { tool, output } => sse_event(
            "tool_result",
            serde_json::json!({ "tool": tool, "output": output }),
        ),
        ChatEvent::Steering {
            message,
            skipped_tools,
        } => sse_event(
            "steering",
            serde_json::json!({ "message": message, "skipped_tools": skipped_tools }),
        ),
        ChatEvent::ApprovalRequired {
            approval_id,
            kind,
            title,
            summary,
            risk_level,
            tool_name,
            arguments,
        } => sse_event(
            "approval_required",
            serde_json::json!({
                "approval_id": approval_id,
                "kind": kind,
                "title": title,
                "summary": summary,
                "risk_level": risk_level,
                "tool_name": tool_name,
                "arguments": arguments,
            }),
        ),
        ChatEvent::ApprovalResolved {
            approval_id,
            status,
        } => sse_event(
            "approval_resolved",
            serde_json::json!({ "approval_id": approval_id, "status": status }),
        ),
        ChatEvent::FilesUploaded(files) => {
            sse_event("files_uploaded", serde_json::json!({ "files": files }))
        }
        ChatEvent::OutputFiles(files) => {
            sse_event("output_files", serde_json::json!({ "files": files }))
        }
        ChatEvent::SkillsUpdated(skills) => sse_event(
            "skills_updated",
            serde_json::json!({ "count": skills.len(), "skills": skills }),
        ),
        ChatEvent::TokenUsage(usage) => {
            sse_event("token_usage", serde_json::json!({ "token_usage": usage }))
        }
        ChatEvent::Done { finish_reason } => sse_event(
            "done",
            serde_json::json!({ "finish_reason": finish_reason }),
        ),
        ChatEvent::Error { detail } => sse_event("error", serde_json::json!({ "detail": detail })),
    }
}

fn sse_event(event: &str, payload: serde_json::Value) -> Event {
    Event::default().event(event).data(payload.to_string())
}
