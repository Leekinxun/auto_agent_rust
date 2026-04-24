use axum::routing::get;
use axum::{Json, Router, extract::State};
use serde_json::json;

use crate::api::errors::ApiResult;
use crate::app_state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new().route("/tasks", get(get_tasks))
}

async fn get_tasks(State(state): State<SharedState>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({
        "tasks": state.task_service.list_all()?
    })))
}
