use serde::{Deserialize, Serialize};

use crate::domain::skills::models::{SkillDocument, SkillHubInstallRecord, SkillScope};

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
pub struct SkillHubInstallRequest {
    pub identifier: String,
    pub source: Option<String>,
    pub name_override: Option<String>,
    pub category: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
pub struct SkillHubInstalledResponse {
    pub installations: Vec<SkillHubInstallRecord>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SkillHubInstallResponse {
    pub skill: SkillDocument,
    pub installation: SkillHubInstallRecord,
    pub skills: Vec<SkillDocument>,
    pub installations: Vec<SkillHubInstallRecord>,
    pub count: usize,
    pub scope: SkillScope,
}

#[derive(Debug, Serialize)]
pub struct SkillHubUninstallResponse {
    pub deleted: SkillDocument,
    pub installation: SkillHubInstallRecord,
    pub skills: Vec<SkillDocument>,
    pub installations: Vec<SkillHubInstallRecord>,
    pub count: usize,
    pub scope: SkillScope,
}
