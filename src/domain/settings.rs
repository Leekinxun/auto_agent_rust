use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SHARED_FRONTEND_SETTINGS_PATH: &str = ".omx/state/frontend-settings.json";
const DEFAULT_BRAND_TITLE: &str = "中科院智能体平台";
const DEFAULT_BRAND_SUBTITLE: &str = "统一承载多模式智能体对话、技能管理与平台配置。";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SharedFrontendSettings {
    pub brand_title: String,
    pub brand_subtitle: String,
    pub mcp_config_path: String,
    pub mcp_base_urls: String,
    pub mcp_disabled_urls: Vec<String>,
    pub mcp_lazy_urls: Vec<String>,
    pub agent_prompt_override: String,
    pub agent_prompt_append: String,
    pub model_id: String,
    pub temperature: String,
    pub max_tokens: String,
    pub max_iterations: String,
    pub top_p: String,
    pub memory_maintenance_system_prompt: String,
    pub memory_maintenance_user_prompt: String,
    pub skill_learning_system_prompt: String,
    pub skill_learning_user_prompt: String,
}

impl Default for SharedFrontendSettings {
    fn default() -> Self {
        Self {
            brand_title: DEFAULT_BRAND_TITLE.to_string(),
            brand_subtitle: DEFAULT_BRAND_SUBTITLE.to_string(),
            mcp_config_path: String::new(),
            mcp_base_urls: String::new(),
            mcp_disabled_urls: Vec::new(),
            mcp_lazy_urls: Vec::new(),
            agent_prompt_override: String::new(),
            agent_prompt_append: String::new(),
            model_id: String::new(),
            temperature: String::new(),
            max_tokens: String::new(),
            max_iterations: String::new(),
            top_p: String::new(),
            memory_maintenance_system_prompt: String::new(),
            memory_maintenance_user_prompt: String::new(),
            skill_learning_system_prompt: String::new(),
            skill_learning_user_prompt: String::new(),
        }
    }
}

pub fn shared_frontend_settings_path(repo_root: &Path) -> PathBuf {
    repo_root.join(SHARED_FRONTEND_SETTINGS_PATH)
}

pub async fn load_shared_frontend_settings(repo_root: &Path) -> Result<SharedFrontendSettings> {
    let path = shared_frontend_settings_path(repo_root);
    if !path.exists() {
        return Ok(SharedFrontendSettings::default());
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let settings = serde_json::from_str::<SharedFrontendSettings>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(settings.normalized())
}

pub async fn save_shared_frontend_settings(
    repo_root: &Path,
    settings: &SharedFrontendSettings,
) -> Result<PathBuf> {
    let path = shared_frontend_settings_path(repo_root);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let encoded = serde_json::to_string_pretty(&settings.normalized())
        .context("failed to encode shared frontend settings")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

impl SharedFrontendSettings {
    pub fn normalized(&self) -> Self {
        Self {
            brand_title: normalize_non_empty(&self.brand_title, DEFAULT_BRAND_TITLE),
            brand_subtitle: normalize_non_empty(&self.brand_subtitle, DEFAULT_BRAND_SUBTITLE),
            mcp_config_path: self.mcp_config_path.trim().to_string(),
            mcp_base_urls: self.mcp_base_urls.trim().to_string(),
            mcp_disabled_urls: normalize_list(&self.mcp_disabled_urls),
            mcp_lazy_urls: normalize_list(&self.mcp_lazy_urls)
                .into_iter()
                .filter(|item| !normalize_list(&self.mcp_disabled_urls).contains(item))
                .collect(),
            agent_prompt_override: self.agent_prompt_override.trim().to_string(),
            agent_prompt_append: self.agent_prompt_append.trim().to_string(),
            model_id: self.model_id.trim().to_string(),
            temperature: self.temperature.trim().to_string(),
            max_tokens: self.max_tokens.trim().to_string(),
            max_iterations: self.max_iterations.trim().to_string(),
            top_p: self.top_p.trim().to_string(),
            memory_maintenance_system_prompt: self
                .memory_maintenance_system_prompt
                .trim()
                .to_string(),
            memory_maintenance_user_prompt: self.memory_maintenance_user_prompt.trim().to_string(),
            skill_learning_system_prompt: self.skill_learning_system_prompt.trim().to_string(),
            skill_learning_user_prompt: self.skill_learning_user_prompt.trim().to_string(),
        }
    }
}

fn normalize_non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_list(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if value.is_empty() || normalized.contains(&value) {
            continue;
        }
        normalized.push(value);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{
        SharedFrontendSettings, load_shared_frontend_settings, save_shared_frontend_settings,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-shared-settings-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time before unix epoch")
                    .as_nanos()
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(&root).expect("create temp repo");
            Self { root }
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn persists_and_loads_shared_frontend_settings() {
        let repo = TestRepo::new();
        let settings = SharedFrontendSettings {
            brand_title: "共享品牌".to_string(),
            brand_subtitle: "共享副标题".to_string(),
            mcp_config_path: " config/mcp.json ".to_string(),
            mcp_base_urls: "http://demo/mcp".to_string(),
            mcp_disabled_urls: vec!["http://a".to_string(), "http://a".to_string()],
            mcp_lazy_urls: vec!["http://b".to_string(), "http://a".to_string()],
            agent_prompt_override: " override base ".to_string(),
            agent_prompt_append: " be concise ".to_string(),
            model_id: "demo-model".to_string(),
            temperature: "0.2".to_string(),
            max_tokens: "4096".to_string(),
            max_iterations: "12".to_string(),
            top_p: "0.9".to_string(),
            memory_maintenance_system_prompt: "sys".to_string(),
            memory_maintenance_user_prompt: "user".to_string(),
            skill_learning_system_prompt: "skill sys".to_string(),
            skill_learning_user_prompt: "skill user".to_string(),
        };

        save_shared_frontend_settings(&repo.root, &settings)
            .await
            .unwrap();
        let loaded = load_shared_frontend_settings(&repo.root).await.unwrap();

        assert_eq!(loaded.brand_title, "共享品牌");
        assert_eq!(loaded.mcp_config_path, "config/mcp.json");
        assert_eq!(loaded.mcp_disabled_urls, vec!["http://a"]);
        assert_eq!(loaded.mcp_lazy_urls, vec!["http://b"]);
        assert_eq!(loaded.agent_prompt_override, "override base");
        assert_eq!(loaded.agent_prompt_append, "be concise");
    }
}
