use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::errors::ApiResult;
use crate::app_state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new().route("/worktrees", get(get_worktrees))
}

async fn get_worktrees(State(state): State<SharedState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({
        "worktrees": state.worktree_service.list_all()?
    })))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

pub fn events_router() -> Router<SharedState> {
    Router::new().route("/events", get(get_events))
}

async fn get_events(
    State(state): State<SharedState>,
    Query(query): Query<LimitQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({
        "events": state.event_service.list_recent(query.limit.unwrap_or(20))?
    })))
}
