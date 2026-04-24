use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::app_state::SharedState;

pub fn router() -> Router<SharedState> {
    Router::new().route("/health", get(health))
}

async fn health(State(state): State<SharedState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "model": state.config.agent.model_id.clone(),
        "workdir": state.repo_root.display().to_string(),
    }))
}
