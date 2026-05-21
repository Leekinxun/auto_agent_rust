use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::config::model::AppConfig;

use super::{
    HarnessAssets, MEMORY_MAINTENANCE_SYSTEM_PATH, MEMORY_MAINTENANCE_USER_TEMPLATE_PATH,
    PromptSource, SKILL_LEARNING_SYSTEM_PATH, SKILL_LEARNING_USER_TEMPLATE_PATH,
    resolve_repo_prompt_source,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSnapshot {
    pub snapshot_id: String,
    pub generated_at_ms: u128,
    pub memory_only_self_evolution: bool,
    pub surfaces: Vec<HarnessSnapshotSurface>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSnapshotSurface {
    pub key: String,
    pub source: PromptSource,
    pub sha1: String,
    pub bytes: usize,
    pub content: String,
}

pub fn build_harness_snapshot(
    repo_root: &Path,
    config: &AppConfig,
    harness: &HarnessAssets,
) -> Result<HarnessSnapshot> {
    let tool_descriptions = serde_json::to_string_pretty(&BTreeMap::from_iter(
        harness
            .tool_descriptions()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    ))
    .context("failed to encode tool description overrides")?;
    let middleware_messages = serde_json::to_string_pretty(&json!({
        "background_results_ack": harness.middleware_messages().background_results_ack,
        "inbox_ack": harness.middleware_messages().inbox_ack,
        "steering_ack": harness.middleware_messages().steering_ack,
        "todo_reminder_user": harness.middleware_messages().todo_reminder_user,
        "todo_reminder_ack": harness.middleware_messages().todo_reminder_ack,
    }))
    .context("failed to encode middleware messages")?;

    let surfaces = vec![
        surface(
            "system.base",
            harness.system_base_template(),
            harness.system_base_source().clone(),
        ),
        surface(
            "subagents.shared",
            harness.subagent_shared_prompt(),
            harness.subagent_shared_source().clone(),
        ),
        surface(
            "subagents.explore",
            harness.subagent_explore_prompt(),
            harness.subagent_explore_source().clone(),
        ),
        surface(
            "subagents.general_purpose",
            harness.subagent_general_prompt(),
            harness.subagent_general_source().clone(),
        ),
        surface(
            "middleware.final_answer_recovery",
            harness.final_answer_recovery_template(),
            harness.final_answer_recovery_source().clone(),
        ),
        surface(
            "memory.maintenance_system",
            &config.memory.prompts.maintenance_system,
            resolve_repo_prompt_source(repo_root, MEMORY_MAINTENANCE_SYSTEM_PATH),
        ),
        surface(
            "memory.maintenance_user_template",
            &config.memory.prompts.maintenance_user_template,
            resolve_repo_prompt_source(repo_root, MEMORY_MAINTENANCE_USER_TEMPLATE_PATH),
        ),
        surface(
            "skills.learning_system",
            &config.skills.prompts.learning_system,
            resolve_repo_prompt_source(repo_root, SKILL_LEARNING_SYSTEM_PATH),
        ),
        surface(
            "skills.learning_user_template",
            &config.skills.prompts.learning_user_template,
            resolve_repo_prompt_source(repo_root, SKILL_LEARNING_USER_TEMPLATE_PATH),
        ),
        surface(
            "tools.descriptions",
            &tool_descriptions,
            harness.tool_descriptions_source().clone(),
        ),
        surface(
            "middleware.messages",
            &middleware_messages,
            harness.middleware_messages_source().clone(),
        ),
    ];

    Ok(HarnessSnapshot {
        snapshot_id: build_snapshot_id(&surfaces)?,
        generated_at_ms: now_ms(),
        memory_only_self_evolution: true,
        surfaces,
    })
}

pub fn current_harness_snapshot_id(
    repo_root: &Path,
    config: &AppConfig,
    harness: &HarnessAssets,
) -> Result<String> {
    Ok(build_harness_snapshot(repo_root, config, harness)?.snapshot_id)
}

fn surface(key: &str, content: &str, source: PromptSource) -> HarnessSnapshotSurface {
    HarnessSnapshotSurface {
        key: key.to_string(),
        source,
        sha1: sha1_hex(content),
        bytes: content.len(),
        content: content.to_string(),
    }
}

fn build_snapshot_id(surfaces: &[HarnessSnapshotSurface]) -> Result<String> {
    let manifest = serde_json::to_string(&json!({
        "memory_only_self_evolution": true,
        "surfaces": surfaces.iter().map(|surface| json!({
            "key": surface.key,
            "source": surface.source,
            "sha1": surface.sha1,
            "bytes": surface.bytes,
        })).collect::<Vec<_>>(),
    }))
    .context("failed to encode harness snapshot manifest")?;
    Ok(format!("hsnap-{}", sha1_hex(&manifest)))
}

fn sha1_hex(content: &str) -> String {
    format!("{:x}", Sha1::digest(content.as_bytes()))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{build_harness_snapshot, current_harness_snapshot_id};
    use crate::config::model::AppConfig;
    use crate::domain::harness::{HarnessAssets, MIDDLEWARE_MESSAGES_PATH, TOOL_DESCRIPTIONS_PATH};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-hsnapshot-{}-{}",
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

    #[test]
    fn builds_snapshot_with_effective_surfaces_and_stable_id() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("harness/system")).unwrap();
        fs::create_dir_all(repo.root.join("harness/memory")).unwrap();
        fs::create_dir_all(repo.root.join("harness/tools")).unwrap();
        fs::create_dir_all(repo.root.join("harness/middleware")).unwrap();
        fs::write(repo.root.join("harness/system/base.md"), "custom base").unwrap();
        fs::write(
            repo.root.join("harness/memory/maintenance_system.md"),
            "custom maintenance system",
        )
        .unwrap();
        fs::write(
            repo.root.join(TOOL_DESCRIPTIONS_PATH),
            r#"{ "read_file": "Custom read" }"#,
        )
        .unwrap();
        fs::write(
            repo.root.join(MIDDLEWARE_MESSAGES_PATH),
            r#"{ "steering_ack": "Custom steering" }"#,
        )
        .unwrap();

        let mut config = AppConfig::default();
        config.memory.prompts.maintenance_system = "custom maintenance system".to_string();
        let harness = HarnessAssets::load(&repo.root).unwrap();

        let snapshot = build_harness_snapshot(&repo.root, &config, &harness).unwrap();

        assert!(snapshot.memory_only_self_evolution);
        assert!(snapshot.snapshot_id.starts_with("hsnap-"));
        assert_eq!(snapshot.snapshot_id.len(), 46);
        assert_eq!(
            snapshot
                .surfaces
                .iter()
                .find(|surface| surface.key == "system.base")
                .unwrap()
                .content,
            "custom base"
        );
        assert_eq!(
            snapshot
                .surfaces
                .iter()
                .find(|surface| surface.key == "memory.maintenance_system")
                .unwrap()
                .source
                .path
                .as_deref(),
            Some("harness/memory/maintenance_system.md")
        );
        assert!(
            snapshot
                .surfaces
                .iter()
                .find(|surface| surface.key == "tools.descriptions")
                .unwrap()
                .content
                .contains("Custom read")
        );
        assert_eq!(
            snapshot.snapshot_id,
            current_harness_snapshot_id(&repo.root, &config, &harness).unwrap()
        );
    }
}
