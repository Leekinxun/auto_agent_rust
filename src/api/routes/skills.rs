use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::dto::skills::{
    SkillDeleteResponse, SkillEvolutionApplyRequest, SkillEvolutionMutationResponse,
    SkillEvolutionQuery, SkillEvolutionStatusResponse, SkillListResponse, SkillMutationResponse,
    SkillUpsertRequest, SkillsQuery,
};
use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::domain::chat::models::SkillPermissions;
use crate::domain::skills::models::{
    DeleteSkillInput, RewritePrivateSkillInput, SaveSkillInput, SkillDocument, SkillScope,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/skills", get(get_skills).post(create_skill))
        .route("/skills/reload", post(reload_skills))
        .route("/skills/{skill_name}/evolution", get(get_skill_evolution))
        .route(
            "/skills/{skill_name}/evolution/reset",
            post(reset_skill_evolution),
        )
        .route(
            "/skills/{skill_name}/evolution/merge",
            post(merge_skill_evolution),
        )
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
    let permissions = parse_skill_permissions(&query)?;
    let items = filter_skills_by_permissions(
        state
            .skill_service
            .list_items(scope, query.user_id.as_deref())?,
        &permissions,
    );
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

fn parse_skill_permissions(query: &SkillsQuery) -> ApiResult<SkillPermissions> {
    Ok(SkillPermissions {
        allowed_skills: parse_string_list(query.allowed_skills.as_deref(), "allowed_skills")?,
        denied_skills: parse_string_list(query.denied_skills.as_deref(), "denied_skills")?,
    })
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

fn skill_allowed(name: &str, permissions: &SkillPermissions) -> bool {
    if permissions
        .denied_skills
        .iter()
        .any(|item| item.trim() == name)
    {
        return false;
    }
    permissions.allowed_skills.is_empty()
        || permissions
            .allowed_skills
            .iter()
            .any(|item| item.trim() == name)
}

fn filter_skills_by_permissions(
    items: Vec<SkillDocument>,
    permissions: &SkillPermissions,
) -> Vec<SkillDocument> {
    items
        .into_iter()
        .filter(|item| skill_allowed(&item.name, permissions))
        .collect()
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

async fn get_skill_evolution(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
    Query(query): Query<SkillEvolutionQuery>,
) -> ApiResult<Json<SkillEvolutionStatusResponse>> {
    Ok(Json(build_skill_evolution_status(
        &state,
        &skill_name,
        &query.user_id,
    )?))
}

async fn reset_skill_evolution(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
    Json(payload): Json<SkillEvolutionApplyRequest>,
) -> ApiResult<Json<SkillEvolutionMutationResponse>> {
    let shared = state.skill_service.get_shared_skill(&skill_name)?;
    let meta = evolution_meta(&shared, "reset", payload.note.as_deref());
    state
        .skill_service
        .reset_private_skill_from_shared(&payload.user_id, &skill_name, &meta)?;
    Ok(Json(SkillEvolutionMutationResponse {
        action: "reset".to_string(),
        status: build_skill_evolution_status(&state, &skill_name, &payload.user_id)?,
    }))
}

async fn merge_skill_evolution(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
    Json(payload): Json<SkillEvolutionApplyRequest>,
) -> ApiResult<Json<SkillEvolutionMutationResponse>> {
    let shared = state.skill_service.get_shared_skill(&skill_name)?;
    if state
        .skill_service
        .get_private_skill(&payload.user_id, &skill_name)
        .is_err()
    {
        let meta = evolution_meta(&shared, "merge", payload.note.as_deref());
        state.skill_service.reset_private_skill_from_shared(
            &payload.user_id,
            &skill_name,
            &meta,
        )?;
    }

    let private = state
        .skill_service
        .get_private_skill(&payload.user_id, &skill_name)?;
    let merged_body = payload
        .body
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(private.body.as_str())
        .to_string();
    let meta = evolution_meta(&shared, "merge", payload.note.as_deref());
    state
        .skill_service
        .rewrite_private_skill(RewritePrivateSkillInput {
            user_id: payload.user_id.clone(),
            name: skill_name.clone(),
            body: merged_body,
            meta,
        })?;

    Ok(Json(SkillEvolutionMutationResponse {
        action: "merge".to_string(),
        status: build_skill_evolution_status(&state, &skill_name, &payload.user_id)?,
    }))
}

fn build_skill_evolution_status(
    state: &SharedState,
    skill_name: &str,
    user_id: &str,
) -> ApiResult<SkillEvolutionStatusResponse> {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Err(ApiError::bad_request("user_id 不能为空"));
    }

    let shared = state.skill_service.get_shared_skill(skill_name).ok();
    let private = state
        .skill_service
        .get_private_skill(user_id, skill_name)
        .ok();
    let effective = state
        .skill_service
        .get_resolved_skill(skill_name, Some(user_id))
        .ok();

    if shared.is_none() && private.is_none() {
        return Err(ApiError::bad_request(format!("Skill 不存在: {skill_name}")));
    }

    let shared_hash = shared.as_ref().map(|skill| skill_body_hash(&skill.body));
    let private_hash = private.as_ref().map(|skill| skill_body_hash(&skill.body));
    let effective_hash = effective.as_ref().map(|skill| skill_body_hash(&skill.body));
    let baseline_hash_when_forked = private
        .as_ref()
        .and_then(|skill| skill.meta.get("coassist_baseline_hash").cloned());
    let has_private = private.is_some();
    let baseline_changed = has_private
        && shared_hash.is_some()
        && baseline_hash_when_forked.as_deref() != shared_hash.as_deref();
    let effective_scope = effective
        .as_ref()
        .map(|skill| skill.scope)
        .unwrap_or(SkillScope::Shared);
    let recommendation = if !has_private {
        "baseline_only"
    } else if baseline_changed {
        "needs_merge_or_reset"
    } else {
        "private_aligned"
    }
    .to_string();

    Ok(SkillEvolutionStatusResponse {
        name: skill_name.to_string(),
        user_id: user_id.to_string(),
        has_private,
        baseline_changed,
        effective_scope,
        shared_hash,
        private_hash,
        effective_hash,
        baseline_hash_when_forked,
        recommendation,
        shared,
        private,
        effective,
    })
}

fn evolution_meta(
    shared: &SkillDocument,
    action: &str,
    note: Option<&str>,
) -> BTreeMap<String, String> {
    let mut meta = shared.meta.clone();
    meta.insert(
        "coassist_baseline_hash".to_string(),
        skill_body_hash(&shared.body),
    );
    meta.insert("coassist_last_action".to_string(), action.to_string());
    meta.insert(
        "coassist_action_at".to_string(),
        now_unix_seconds().to_string(),
    );
    if let Some(note) = note.map(str::trim).filter(|value| !value.is_empty()) {
        meta.insert("coassist_action_note".to_string(), note.to_string());
    }
    meta
}

fn skill_body_hash(body: &str) -> String {
    format!("{:x}", Sha1::digest(body.trim().as_bytes()))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}
