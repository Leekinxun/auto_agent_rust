use axum::extract::{Path, Query, State};
use axum::{Json, Router};
use serde::Deserialize;

use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::domain::chat::compaction::read_context_transcript;
use crate::domain::run_capture::{
    AgentRunDetail, AgentRunManifest, AgentRunReplay, list_agent_run_manifests,
    read_agent_run_detail, replay_agent_run,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/runs", axum::routing::get(list_runs))
        .route(
            "/runs/{user_id}/{run_id}",
            axum::routing::get(get_run_detail),
        )
        .route(
            "/runs/{user_id}/{run_id}/replay",
            axum::routing::post(replay_run),
        )
        .route(
            "/runs/{user_id}/{run_id}/transcripts/{transcript_id}",
            axum::routing::get(get_run_transcript),
        )
}

#[derive(Debug, Deserialize)]
struct RunListQuery {
    user_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ReplayQuery {
    from_step_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptQuery {
    limit: Option<usize>,
}

async fn list_runs(
    State(state): State<SharedState>,
    Query(query): Query<RunListQuery>,
) -> ApiResult<Json<Vec<AgentRunManifest>>> {
    Ok(Json(
        list_agent_run_manifests(
            &state.repo_root,
            query.user_id.as_deref(),
            query.limit.unwrap_or(50),
        )
        .await?,
    ))
}

async fn get_run_detail(
    State(state): State<SharedState>,
    Path((user_id, run_id)): Path<(String, String)>,
) -> ApiResult<Json<AgentRunDetail>> {
    let detail = read_agent_run_detail(&state.repo_root, &user_id, &run_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("run not found: {run_id}")))?;
    Ok(Json(detail))
}

async fn replay_run(
    State(state): State<SharedState>,
    Path((user_id, run_id)): Path<(String, String)>,
    Query(query): Query<ReplayQuery>,
) -> ApiResult<Json<AgentRunReplay>> {
    let replay = replay_agent_run(
        &state.repo_root,
        &user_id,
        &run_id,
        query.from_step_index.unwrap_or(0),
    )
    .await?
    .ok_or_else(|| ApiError::not_found(format!("run not found: {run_id}")))?;
    Ok(Json(replay))
}

async fn get_run_transcript(
    State(state): State<SharedState>,
    Path((_user_id, _run_id, transcript_id)): Path<(String, String, String)>,
    Query(query): Query<TranscriptQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let content = read_context_transcript(
        &state.repo_root,
        &serde_json::json!({
            "transcript_id": transcript_id,
            "limit": query.limit,
        }),
    )
    .await?;
    let value = serde_json::from_str(&content)
        .map_err(|error| ApiError::Internal(format!("invalid transcript payload: {error}")))?;
    Ok(Json(value))
}
