use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::config::model::AppConfig;
use crate::domain::harness::{
    MEMORY_MAINTENANCE_SYSTEM_PATH, MEMORY_MAINTENANCE_USER_TEMPLATE_PATH,
    SKILL_LEARNING_SYSTEM_PATH, SKILL_LEARNING_USER_TEMPLATE_PATH,
};

pub fn find_repo_root() -> Result<PathBuf> {
    if let Ok(value) = env::var("REPO_ROOT") {
        let path = PathBuf::from(value);
        if path.join("config").join("config.yaml").exists() {
            return Ok(path);
        }
    }

    let current_dir = env::current_dir().context("failed to read current dir")?;
    for candidate in current_dir.ancestors() {
        if candidate.join("config").join("config.yaml").exists() {
            return Ok(candidate.to_path_buf());
        }
    }

    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in manifest_root.ancestors() {
        if candidate.join("config").join("config.yaml").exists() {
            return Ok(candidate.to_path_buf());
        }
    }

    bail!("config/config.yaml not found in current or manifest ancestors")
}

pub fn load_config(repo_root: &Path) -> Result<AppConfig> {
    let config_path = repo_root.join("config").join("config.yaml");
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut config: AppConfig =
        serde_yaml::from_str(&raw).context("invalid yaml in config/config.yaml")?;
    apply_env_overrides(&mut config)?;
    apply_prompt_file_overrides(repo_root, &mut config)?;
    Ok(config)
}

pub fn load_repo_dotenv(repo_root: &Path) -> Result<Option<PathBuf>> {
    let dotenv_path = repo_root.join(".env");
    if !dotenv_path.exists() {
        return Ok(None);
    }

    dotenvy::from_path(&dotenv_path)
        .with_context(|| format!("failed to load {}", dotenv_path.display()))?;

    Ok(Some(dotenv_path))
}

fn apply_env_overrides(config: &mut AppConfig) -> Result<()> {
    apply_env_overrides_with(config, &|key| env::var(key).ok())
}

fn apply_env_overrides_with<F>(config: &mut AppConfig, get_env: &F) -> Result<()>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_MODEL_ID", "MODEL_ID"]) {
        config.agent.model_id = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_BASE_URL", "ANTHROPIC_BASE_URL"]) {
        config.agent.base_url = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_API_KEY"]) {
        config.agent.api_key = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_MAX_TOKENS"]) {
        config.agent.max_tokens = parse_positive_u32("AGENT_MAX_TOKENS", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_TEMPERATURE"]) {
        config.agent.temperature = Some(parse_temperature("AGENT_TEMPERATURE", &value)?);
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_TOP_P"]) {
        config.agent.top_p = Some(parse_top_p("AGENT_TOP_P", &value)?);
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_MAX_ITERATIONS"]) {
        config.agent.max_iterations = parse_positive_usize("AGENT_MAX_ITERATIONS", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_SUBAGENT_MAX_ITERATIONS"]) {
        config.agent.subagent_max_iterations =
            parse_positive_usize("AGENT_SUBAGENT_MAX_ITERATIONS", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["AGENT_AUTO_COMPACT_TOKEN_THRESHOLD"]) {
        config.agent.auto_compact_token_threshold =
            parse_positive_usize("AGENT_AUTO_COMPACT_TOKEN_THRESHOLD", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["SERVER_HOST"]) {
        config.server.host = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["SERVER_PORT"]) {
        config.server.port = parse_env::<u16>("SERVER_PORT", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["CORS_ALLOW_ORIGINS"]) {
        config.server.cors.allow_origins = split_csv(&value);
    }
    if let Some(value) = first_non_empty_env(get_env, &["CORS_ALLOW_CREDENTIALS"]) {
        config.server.cors.allow_credentials = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    if let Some(value) = first_non_empty_env(get_env, &["CORS_ALLOW_METHODS"]) {
        config.server.cors.allow_methods = split_csv(&value);
    }
    if let Some(value) = first_non_empty_env(get_env, &["CORS_ALLOW_HEADERS"]) {
        config.server.cors.allow_headers = split_csv(&value);
    }
    if let Some(value) = first_non_empty_env(get_env, &["MCP_BASE_URL"]) {
        config.mcp.base_url = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["MCP_BASE_URLS"]) {
        config.mcp.base_urls = split_csv(&value);
    }
    if let Some(value) = first_non_empty_env(get_env, &["MCP_CONFIG_PATH"]) {
        config.mcp.config_path = value;
    }
    if let Some(value) = first_non_empty_env(get_env, &["MCP_TIMEOUT"]) {
        config.mcp.timeout = parse_positive_u64("MCP_TIMEOUT", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["MCP_CONNECT_TIMEOUT"]) {
        config.mcp.connect_timeout = parse_positive_u64("MCP_CONNECT_TIMEOUT", &value)?;
    }
    if let Some(value) = first_non_empty_env(get_env, &["LOG_DIR"]) {
        config.logging.dir = value;
    }

    Ok(())
}

fn apply_prompt_file_overrides(repo_root: &Path, config: &mut AppConfig) -> Result<()> {
    if let Some(content) = read_optional_prompt_file(repo_root, MEMORY_MAINTENANCE_SYSTEM_PATH)? {
        config.memory.prompts.maintenance_system = content;
    }
    if let Some(content) =
        read_optional_prompt_file(repo_root, MEMORY_MAINTENANCE_USER_TEMPLATE_PATH)?
    {
        config.memory.prompts.maintenance_user_template = content;
    }
    if let Some(content) = read_optional_prompt_file(repo_root, SKILL_LEARNING_SYSTEM_PATH)? {
        config.skills.prompts.learning_system = content;
    }
    if let Some(content) = read_optional_prompt_file(repo_root, SKILL_LEARNING_USER_TEMPLATE_PATH)?
    {
        config.skills.prompts.learning_user_template = content;
    }
    Ok(())
}

fn read_optional_prompt_file(repo_root: &Path, relative_path: &str) -> Result<Option<String>> {
    let path = repo_root.join(relative_path);
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    ensure!(!content.trim().is_empty(), "{} is empty", path.display());
    Ok(Some(content))
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn first_non_empty_env<F>(get_env: &F, keys: &[&str]) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    keys.iter().find_map(|key| {
        get_env(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn parse_env<T>(key: &str, raw: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.parse::<T>()
        .map_err(|err| anyhow::anyhow!("invalid {key} value: {raw} ({err})"))
}

fn parse_positive_u32(key: &str, raw: &str) -> Result<u32> {
    let value = parse_env::<u32>(key, raw)?;
    ensure!(value > 0, "{key} must be greater than 0");
    Ok(value)
}

fn parse_positive_u64(key: &str, raw: &str) -> Result<u64> {
    let value = parse_env::<u64>(key, raw)?;
    ensure!(value > 0, "{key} must be greater than 0");
    Ok(value)
}

fn parse_positive_usize(key: &str, raw: &str) -> Result<usize> {
    let value = parse_env::<usize>(key, raw)?;
    ensure!(value > 0, "{key} must be greater than 0");
    Ok(value)
}

fn parse_temperature(key: &str, raw: &str) -> Result<f32> {
    let value = parse_env::<f32>(key, raw)?;
    ensure!((0.0..=2.0).contains(&value), "{key} must be within 0 and 2");
    Ok(value)
}

fn parse_top_p(key: &str, raw: &str) -> Result<f32> {
    let value = parse_env::<f32>(key, raw)?;
    ensure!(value > 0.0 && value <= 1.0, "{key} must be within 0 and 1");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn env_overrides_support_aliases_and_ignore_blank_values() {
        let mut config = AppConfig::default();
        let env = env_map(&[
            ("MODEL_ID", "legacy-model"),
            ("AGENT_MODEL_ID", "modern-model"),
            ("ANTHROPIC_BASE_URL", "   "),
            ("AGENT_BASE_URL", " http://llm.example/v1 "),
            ("AGENT_API_KEY", " secret "),
            ("AGENT_MAX_TOKENS", "9000"),
            ("AGENT_TEMPERATURE", "0.3"),
            ("AGENT_TOP_P", "0.9"),
            ("AGENT_MAX_ITERATIONS", "10"),
            ("AGENT_SUBAGENT_MAX_ITERATIONS", "25"),
            ("AGENT_AUTO_COMPACT_TOKEN_THRESHOLD", "45000"),
            ("SERVER_HOST", "127.0.0.1"),
            ("SERVER_PORT", "19000"),
            ("CORS_ALLOW_ORIGINS", "http://a.example, http://b.example"),
            ("CORS_ALLOW_CREDENTIALS", "true"),
            ("CORS_ALLOW_METHODS", "GET, POST"),
            ("CORS_ALLOW_HEADERS", "Authorization, Content-Type"),
            ("MCP_BASE_URL", "http://mcp.example/mcp"),
            ("MCP_TIMEOUT", "120"),
            ("MCP_CONNECT_TIMEOUT", "5"),
            ("LOG_DIR", "runtime-logs"),
        ]);

        apply_env_overrides_with(&mut config, &|key| env.get(key).cloned()).unwrap();

        assert_eq!(config.agent.model_id, "modern-model");
        assert_eq!(config.agent.base_url, "http://llm.example/v1");
        assert_eq!(config.agent.api_key, "secret");
        assert_eq!(config.agent.max_tokens, 9000);
        assert_eq!(config.agent.temperature, Some(0.3));
        assert_eq!(config.agent.top_p, Some(0.9));
        assert_eq!(config.agent.max_iterations, 10);
        assert_eq!(config.agent.subagent_max_iterations, 25);
        assert_eq!(config.agent.auto_compact_token_threshold, 45_000);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 19000);
        assert_eq!(
            config.server.cors.allow_origins,
            vec!["http://a.example", "http://b.example"]
        );
        assert!(config.server.cors.allow_credentials);
        assert_eq!(config.server.cors.allow_methods, vec!["GET", "POST"]);
        assert_eq!(
            config.server.cors.allow_headers,
            vec!["Authorization", "Content-Type"]
        );
        assert_eq!(config.mcp.base_url, "http://mcp.example/mcp");
        assert_eq!(config.mcp.timeout, 120);
        assert_eq!(config.mcp.connect_timeout, 5);
        assert_eq!(config.logging.dir, "runtime-logs");
    }

    #[test]
    fn legacy_env_names_still_work() {
        let mut config = AppConfig::default();
        let env = env_map(&[
            ("MODEL_ID", "legacy-model"),
            ("ANTHROPIC_BASE_URL", "http://legacy.example/v1"),
            ("MCP_BASE_URL", "http://legacy.example/mcp"),
        ]);

        apply_env_overrides_with(&mut config, &|key| env.get(key).cloned()).unwrap();

        assert_eq!(config.agent.model_id, "legacy-model");
        assert_eq!(config.agent.base_url, "http://legacy.example/v1");
        assert_eq!(config.mcp.base_url, "http://legacy.example/mcp");
    }

    #[test]
    fn invalid_numeric_or_sampling_overrides_fail_fast() {
        let mut config = AppConfig::default();
        let env = env_map(&[
            ("AGENT_MAX_TOKENS", "0"),
            ("AGENT_TEMPERATURE", "2.5"),
            ("AGENT_TOP_P", "1.2"),
        ]);

        let err = apply_env_overrides_with(&mut config, &|key| env.get(key).cloned())
            .expect_err("invalid env overrides should fail");
        let message = format!("{err:#}");

        assert!(message.contains("AGENT_MAX_TOKENS"));
    }

    fn env_map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn missing_repo_dotenv_is_ignored() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let repo_root = std::env::temp_dir().join(format!("auto-claude-missing-dotenv-{unique}"));
        std::fs::create_dir_all(&repo_root).unwrap();

        let loaded = load_repo_dotenv(&repo_root).unwrap();

        assert!(loaded.is_none());

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn harness_prompt_files_override_config_prompt_strings() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let repo_root = std::env::temp_dir().join(format!("auto-claude-harness-prompts-{unique}"));
        std::fs::create_dir_all(repo_root.join("harness/memory")).unwrap();
        std::fs::create_dir_all(repo_root.join("harness/skills")).unwrap();
        std::fs::write(
            repo_root.join("harness/memory/maintenance_system.md"),
            "memory system from file",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("harness/memory/maintenance_user_template.md"),
            "memory template from file",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("harness/skills/learning_system.md"),
            "skill system from file",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("harness/skills/learning_user_template.md"),
            "skill template from file",
        )
        .unwrap();

        let mut config = AppConfig::default();
        apply_prompt_file_overrides(&repo_root, &mut config).unwrap();

        assert_eq!(
            config.memory.prompts.maintenance_system,
            "memory system from file"
        );
        assert_eq!(
            config.memory.prompts.maintenance_user_template,
            "memory template from file"
        );
        assert_eq!(
            config.skills.prompts.learning_system,
            "skill system from file"
        );
        assert_eq!(
            config.skills.prompts.learning_user_template,
            "skill template from file"
        );

        let _ = std::fs::remove_dir_all(&repo_root);
    }
}
