use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};

use crate::api::dto::skills::{
    SkillDeleteResponse, SkillListResponse, SkillMutationResponse, SkillUpsertRequest, SkillsQuery,
};
use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::domain::skills::models::{DeleteSkillInput, SaveSkillInput, SkillScope};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/skills", get(get_skills).post(create_skill))
        .route("/skills/reload", post(reload_skills))
        .route(
            "/skills/{skill_name}",
            put(update_skill).delete(delete_skill),
        )
}

async fn get_skills(
    State(state): State<SharedState>,
    Query(query): Query<SkillsQuery>,
) -> ApiResult<Json<SkillListResponse>> {
    let scope = SkillScope::normalize(query.scope.as_deref(), query.user_id.as_deref(), true)
        .map_err(ApiError::bad_request)?;
    let items = state
        .skill_service
        .list_items(scope, query.user_id.as_deref())?;
    let descriptions = state.skill_service.render_descriptions(&items);

    Ok(Json(SkillListResponse {
        count: items.len(),
        names: items.iter().map(|item| item.name.clone()).collect(),
        descriptions,
        skills: items,
        scope,
        user_id: query.user_id,
    }))
}

async fn create_skill(
    State(state): State<SharedState>,
    Json(payload): Json<SkillUpsertRequest>,
) -> ApiResult<Json<SkillMutationResponse>> {
    let scope = SkillScope::normalize(payload.scope.as_deref(), payload.user_id.as_deref(), false)
        .map_err(ApiError::bad_request)?;

    let skill = state.skill_service.save_skill(SaveSkillInput {
        current_name: None,
        name: payload.name,
        description: payload.description,
        tags: payload.tags,
        trigger: payload.trigger,
        body: payload.body,
        folder: payload.folder,
        scope,
        user_id: payload.user_id.clone(),
    })?;
    let items = state
        .skill_service
        .list_items(scope, payload.user_id.as_deref())?;

    Ok(Json(SkillMutationResponse {
        count: items.len(),
        skill,
        skills: items,
        scope,
        user_id: payload.user_id,
    }))
}

async fn update_skill(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
    Json(payload): Json<SkillUpsertRequest>,
) -> ApiResult<Json<SkillMutationResponse>> {
    let scope = SkillScope::normalize(payload.scope.as_deref(), payload.user_id.as_deref(), false)
        .map_err(ApiError::bad_request)?;

    let skill = state.skill_service.save_skill(SaveSkillInput {
        current_name: Some(skill_name),
        name: payload.name,
        description: payload.description,
        tags: payload.tags,
        trigger: payload.trigger,
        body: payload.body,
        folder: payload.folder,
        scope,
        user_id: payload.user_id.clone(),
    })?;
    let items = state
        .skill_service
        .list_items(scope, payload.user_id.as_deref())?;

    Ok(Json(SkillMutationResponse {
        count: items.len(),
        skill,
        skills: items,
        scope,
        user_id: payload.user_id,
    }))
}

async fn delete_skill(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
    Query(query): Query<SkillsQuery>,
) -> ApiResult<Json<SkillDeleteResponse>> {
    let scope = SkillScope::normalize(query.scope.as_deref(), query.user_id.as_deref(), false)
        .map_err(ApiError::bad_request)?;

    let deleted = state.skill_service.delete_skill(DeleteSkillInput {
        name: skill_name,
        scope,
        user_id: query.user_id.clone(),
    })?;
    let items = state
        .skill_service
        .list_items(scope, query.user_id.as_deref())?;

    Ok(Json(SkillDeleteResponse {
        deleted,
        count: items.len(),
        skills: items,
        scope,
        user_id: query.user_id,
    }))
}

async fn reload_skills(State(state): State<SharedState>) -> ApiResult<Json<SkillListResponse>> {
    let items = state.skill_service.list_items(SkillScope::Shared, None)?;
    let descriptions = state.skill_service.render_descriptions(&items);

    Ok(Json(SkillListResponse {
        count: items.len(),
        names: items.iter().map(|item| item.name.clone()).collect(),
        descriptions,
        skills: items,
        scope: SkillScope::Shared,
        user_id: None,
    }))
}
