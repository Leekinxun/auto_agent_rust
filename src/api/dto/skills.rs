use serde::{Deserialize, Serialize};

use crate::domain::skills::models::{SkillDocument, SkillScope};

#[derive(Debug, Deserialize)]
pub struct SkillsQuery {
    pub scope: Option<String>,
    pub user_id: Option<String>,
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
