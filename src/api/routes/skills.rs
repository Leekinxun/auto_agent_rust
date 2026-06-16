use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};

use crate::api::dto::skills::{
    SkillDeleteResponse, SkillHubInstallRequest, SkillHubInstallResponse,
    SkillHubInstalledResponse, SkillHubUninstallResponse, SkillListResponse, SkillMutationResponse,
    SkillUpsertRequest, SkillsQuery,
};
use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::domain::chat::models::SkillPermissions;
use crate::domain::skills::models::{
    DeleteSkillInput, InstallHubSkillInput, SaveSkillInput, SkillDocument, SkillScope,
};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/skills", get(get_skills).post(create_skill))
        .route("/skills/reload", post(reload_skills))
        .route("/skills/hub/installed", get(get_hub_installed_skills))
        .route("/skills/hub/install", post(install_hub_skill))
        .route(
            "/skills/hub/{skill_name}",
            axum::routing::delete(uninstall_hub_skill),
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

    let skill = state
        .skill_service
        .save_skill(SaveSkillInput {
            current_name: None,
            name: payload.name,
            description: payload.description,
            tags: payload.tags,
            trigger: payload.trigger,
            body: payload.body,
            folder: payload.folder,
            scope,
            user_id: payload.user_id.clone(),
        })
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
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

    let skill = state
        .skill_service
        .save_skill(SaveSkillInput {
            current_name: Some(skill_name),
            name: payload.name,
            description: payload.description,
            tags: payload.tags,
            trigger: payload.trigger,
            body: payload.body,
            folder: payload.folder,
            scope,
            user_id: payload.user_id.clone(),
        })
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
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

    let deleted = state
        .skill_service
        .delete_skill(DeleteSkillInput {
            name: skill_name,
            scope,
            user_id: query.user_id.clone(),
        })
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
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

async fn get_hub_installed_skills(
    State(state): State<SharedState>,
) -> ApiResult<Json<SkillHubInstalledResponse>> {
    let installations = state.skill_service.list_hub_installations()?;
    Ok(Json(SkillHubInstalledResponse {
        count: installations.len(),
        installations,
    }))
}

async fn install_hub_skill(
    State(state): State<SharedState>,
    Json(payload): Json<SkillHubInstallRequest>,
) -> ApiResult<Json<SkillHubInstallResponse>> {
    if payload.identifier.trim().is_empty() {
        return Err(ApiError::bad_request("identifier 不能为空"));
    }
    let result = state
        .skill_service
        .install_hub_skill(InstallHubSkillInput {
            identifier: payload.identifier,
            source: payload.source,
            name_override: payload.name_override,
            category: payload.category,
            force: payload.force,
        })
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let items = state.skill_service.list_items(SkillScope::Shared, None)?;
    let installations = state.skill_service.list_hub_installations()?;

    Ok(Json(SkillHubInstallResponse {
        count: items.len(),
        skill: result.skill,
        installation: result.installation,
        skills: items,
        installations,
        scope: SkillScope::Shared,
    }))
}

async fn uninstall_hub_skill(
    State(state): State<SharedState>,
    Path(skill_name): Path<String>,
) -> ApiResult<Json<SkillHubUninstallResponse>> {
    let result = state
        .skill_service
        .uninstall_hub_skill(&skill_name)
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let items = state.skill_service.list_items(SkillScope::Shared, None)?;
    let installations = state.skill_service.list_hub_installations()?;

    Ok(Json(SkillHubUninstallResponse {
        count: items.len(),
        deleted: result.deleted,
        installation: result.installation,
        skills: items,
        installations,
        scope: SkillScope::Shared,
    }))
}
