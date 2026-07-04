use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::api::dto::memory::{
    MemoryQuery, MemoryResponse, SessionDeleteResponse, UserMemoryResetResponse,
};
use crate::api::errors::ApiResult;
use crate::app_state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/memory", get(get_memory))
        .route("/memory/user/{user_id}", delete(reset_user_memory))
        .route(
            "/memory/session/{session_id}",
            delete(delete_session_memory),
        )
}

async fn get_memory(
    State(state): State<SharedState>,
    Query(query): Query<MemoryQuery>,
) -> ApiResult<Json<MemoryResponse>> {
    let Some(user_id) = query.effective_user_id() else {
        return Ok(Json(MemoryResponse {
            user_id: None,
            user_md: String::new(),
            memory_md: String::new(),
            count: 0,
        }));
    };

    let snapshot = state.memory_service.load_snapshot(user_id)?;
    let count =
        usize::from(!snapshot.user_md.is_empty()) + usize::from(!snapshot.memory_md.is_empty());

    Ok(Json(MemoryResponse {
        user_id: Some(snapshot.user_id),
        user_md: snapshot.user_md,
        memory_md: snapshot.memory_md,
        count,
    }))
}

async fn delete_session_memory(
    State(state): State<SharedState>,
    Path(session_id): Path<String>,
) -> Json<SessionDeleteResponse> {
    state.session_service.delete(&session_id);
    Json(SessionDeleteResponse {
        success: true,
        session_id,
    })
}

async fn reset_user_memory(
    State(state): State<SharedState>,
    Path(user_id): Path<String>,
) -> ApiResult<Json<UserMemoryResetResponse>> {
    let reset = state.memory_service.reset_user_memory(&user_id)?;
    let cached_session_snapshots_cleared = state.session_service.clear_cached_snapshots(&user_id);
    Ok(Json(UserMemoryResetResponse {
        success: true,
        user_id: reset.user_id,
        user_md_cleared: reset.user_md_cleared,
        memory_md_cleared: reset.memory_md_cleared,
        private_skills_cleared: reset.private_skills_cleared,
        private_skill_count: reset.private_skill_count,
        cached_session_snapshots_cleared,
        mcp_state: "stateless_request_scoped".to_string(),
        user_md_path: reset.paths.user_md.display().to_string(),
        memory_md_path: reset.paths.memory_md.display().to_string(),
        private_skills_path: reset.paths.skills_dir.display().to_string(),
    }))
}
