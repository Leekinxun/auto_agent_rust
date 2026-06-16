use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillScope {
    Shared,
    Private,
    Effective,
}

impl SkillScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Private => "private",
            Self::Effective => "effective",
        }
    }

    pub fn normalize(
        scope: Option<&str>,
        user_id: Option<&str>,
        allow_effective: bool,
    ) -> Result<Self, String> {
        let value = scope.unwrap_or_default().trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(if allow_effective && user_id.is_some() {
                Self::Effective
            } else {
                Self::Shared
            });
        }

        match value.as_str() {
            "shared" => Ok(Self::Shared),
            "private" => {
                if user_id.is_none() {
                    Err("私有 skill 需要提供 user_id".to_string())
                } else {
                    Ok(Self::Private)
                }
            }
            "effective" => {
                if !allow_effective {
                    Err("当前接口不支持 effective scope".to_string())
                } else if user_id.is_none() {
                    Ok(Self::Shared)
                } else {
                    Ok(Self::Effective)
                }
            }
            _ => Err(format!("不支持的 skill scope: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDocument {
    pub name: String,
    pub description: String,
    pub tags: String,
    pub trigger: String,
    pub folder: String,
    pub path: String,
    pub body: String,
    pub meta: BTreeMap<String, String>,
    pub scope: SkillScope,
}

#[derive(Debug, Clone)]
pub struct SaveSkillInput {
    pub current_name: Option<String>,
    pub name: String,
    pub description: String,
    pub tags: String,
    pub trigger: String,
    pub body: String,
    pub folder: Option<String>,
    pub scope: SkillScope,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteSkillInput {
    pub name: String,
    pub scope: SkillScope,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallHubSkillInput {
    pub identifier: String,
    pub source: Option<String>,
    pub name_override: Option<String>,
    pub category: Option<String>,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubInstallRecord {
    pub name: String,
    pub source: String,
    pub identifier: String,
    pub trust_level: String,
    pub scan_verdict: String,
    pub content_hash: String,
    pub install_path: String,
    pub files: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub installed_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct InstallHubSkillResult {
    pub skill: SkillDocument,
    pub installation: SkillHubInstallRecord,
}

#[derive(Debug, Clone)]
pub struct UninstallHubSkillResult {
    pub deleted: SkillDocument,
    pub installation: SkillHubInstallRecord,
}
