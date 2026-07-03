use serde::{Deserialize, Serialize};

use crate::domain::skills::models::{SkillDocument, SkillScope};

#[derive(Debug, Deserialize)]
pub struct SkillsQuery {
    pub scope: Option<String>,
    pub user_id: Option<String>,
    pub allowed_skills: Option<String>,
    pub denied_skills: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SkillUpsertRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub body: String,
    pub folder: Option<String>,
    pub scope: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillListResponse {
    pub skills: Vec<SkillDocument>,
    pub count: usize,
    pub names: Vec<String>,
    pub descriptions: String,
    pub scope: SkillScope,
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillMutationResponse {
    pub skill: SkillDocument,
    pub skills: Vec<SkillDocument>,
    pub count: usize,
    pub scope: SkillScope,
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillDeleteResponse {
    pub deleted: SkillDocument,
    pub skills: Vec<SkillDocument>,
    pub count: usize,
    pub scope: SkillScope,
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SkillEvolutionQuery {
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SkillEvolutionApplyRequest {
    pub user_id: String,
    pub body: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillEvolutionStatusResponse {
    pub name: String,
    pub user_id: String,
    pub has_private: bool,
    pub baseline_changed: bool,
    pub effective_scope: SkillScope,
    pub shared_hash: Option<String>,
    pub private_hash: Option<String>,
    pub effective_hash: Option<String>,
    pub baseline_hash_when_forked: Option<String>,
    pub recommendation: String,
    pub shared: Option<SkillDocument>,
    pub private: Option<SkillDocument>,
    pub effective: Option<SkillDocument>,
}

#[derive(Debug, Serialize)]
pub struct SkillEvolutionMutationResponse {
    pub action: String,
    pub status: SkillEvolutionStatusResponse,
}
