use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::config::model::AppConfig;

use super::snapshot::{HarnessSnapshot, build_harness_snapshot};
use super::{
    FINAL_ANSWER_RECOVERY_PATH, HarnessAssets, MEMORY_MAINTENANCE_SYSTEM_PATH,
    MEMORY_MAINTENANCE_USER_TEMPLATE_PATH, MIDDLEWARE_MESSAGES_PATH, SKILL_LEARNING_SYSTEM_PATH,
    SKILL_LEARNING_USER_TEMPLATE_PATH, SUBAGENT_EXPLORE_PATH, SUBAGENT_GENERAL_PATH,
    SUBAGENT_SHARED_PATH, SYSTEM_BASE_PATH, TOOL_DESCRIPTIONS_PATH,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSurfaceEdit {
    pub surface_key: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedHarnessSurface {
    pub key: String,
    pub path: String,
    pub sha1: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessPreviewSurface {
    pub surface_key: String,
    pub path: String,
    pub changed: bool,
    pub before_sha1: String,
    pub after_sha1: String,
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub byte_delta: i64,
    pub before_lines: usize,
    pub after_lines: usize,
    pub line_delta: i64,
    pub before_content: String,
    pub after_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessApplyPreview {
    pub snapshot_before: HarnessSnapshot,
    pub expected_snapshot_id: Option<String>,
    pub changed_surface_count: usize,
    pub surfaces: Vec<HarnessPreviewSurface>,
}

#[derive(Debug, Clone)]
pub struct HarnessApplyExecution {
    pub snapshot_before: HarnessSnapshot,
    pub snapshot_after: HarnessSnapshot,
    pub preview: HarnessApplyPreview,
    pub updated_surfaces: Vec<AppliedHarnessSurface>,
}

pub fn supported_surface_path(key: &str) -> Option<&'static str> {
    match key {
        "system.base" => Some(SYSTEM_BASE_PATH),
        "subagents.shared" => Some(SUBAGENT_SHARED_PATH),
        "subagents.explore" => Some(SUBAGENT_EXPLORE_PATH),
        "subagents.general_purpose" => Some(SUBAGENT_GENERAL_PATH),
        "middleware.final_answer_recovery" => Some(FINAL_ANSWER_RECOVERY_PATH),
        "memory.maintenance_system" => Some(MEMORY_MAINTENANCE_SYSTEM_PATH),
        "memory.maintenance_user_template" => Some(MEMORY_MAINTENANCE_USER_TEMPLATE_PATH),
        "skills.learning_system" => Some(SKILL_LEARNING_SYSTEM_PATH),
        "skills.learning_user_template" => Some(SKILL_LEARNING_USER_TEMPLATE_PATH),
        "tools.descriptions" => Some(TOOL_DESCRIPTIONS_PATH),
        "middleware.messages" => Some(MIDDLEWARE_MESSAGES_PATH),
        _ => None,
    }
}

pub fn preview_harness_edits(
    repo_root: &Path,
    config: &AppConfig,
    harness: &HarnessAssets,
    edits: &[HarnessSurfaceEdit],
    expected_snapshot_id: Option<&str>,
) -> Result<HarnessApplyPreview> {
    let snapshot_before = build_harness_snapshot(repo_root, config, harness)?;
    if let Some(expected_snapshot_id) = expected_snapshot_id {
        ensure!(
            snapshot_before.snapshot_id == expected_snapshot_id,
            "snapshot mismatch: expected {}, current {}",
            expected_snapshot_id,
            snapshot_before.snapshot_id
        );
    }

    let edits = normalize_edits(edits)?;
    ensure!(
        !edits.is_empty(),
        "at least one supported surface edit is required"
    );

    let surface_by_key = snapshot_before
        .surfaces
        .iter()
        .map(|surface| (surface.key.as_str(), surface))
        .collect::<HashMap<_, _>>();

    let surfaces = edits
        .into_iter()
        .map(|edit| {
            validate_surface_content(&edit.surface_key, &edit.content)?;
            let before = surface_by_key
                .get(edit.surface_key.as_str())
                .copied()
                .with_context(|| format!("missing snapshot surface for {}", edit.surface_key))?;
            let path = supported_surface_path(&edit.surface_key)
                .expect("supported path must exist after normalization")
                .to_string();
            let before_lines = count_lines(&before.content);
            let after_lines = count_lines(&edit.content);
            let after_sha1 = sha1_hex(&edit.content);
            let after_bytes = edit.content.len();

            Ok(HarnessPreviewSurface {
                surface_key: edit.surface_key,
                path,
                changed: before.content != edit.content,
                before_sha1: before.sha1.clone(),
                after_sha1,
                before_bytes: before.bytes,
                after_bytes,
                byte_delta: after_bytes as i64 - before.bytes as i64,
                before_lines,
                after_lines,
                line_delta: after_lines as i64 - before_lines as i64,
                before_content: before.content.clone(),
                after_content: edit.content,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let changed_surface_count = surfaces.iter().filter(|surface| surface.changed).count();

    Ok(HarnessApplyPreview {
        snapshot_before,
        expected_snapshot_id: expected_snapshot_id.map(str::to_string),
        changed_surface_count,
        surfaces,
    })
}

pub fn apply_harness_edits(
    repo_root: &Path,
    config: &AppConfig,
    harness: &HarnessAssets,
    edits: &[HarnessSurfaceEdit],
    expected_snapshot_id: Option<&str>,
) -> Result<HarnessApplyExecution> {
    let preview = preview_harness_edits(repo_root, config, harness, edits, expected_snapshot_id)?;
    let snapshot_before = preview.snapshot_before.clone();

    for surface in &preview.surfaces {
        let path = repo_root.join(&surface.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, &surface.after_content)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    let config_path = repo_root.join("config").join("config.yaml");
    let refreshed_config = if config_path.exists() {
        crate::config::loader::load_config(repo_root)?
    } else {
        config.clone()
    };
    let refreshed_harness = HarnessAssets::load(repo_root)?;
    let snapshot_after = build_harness_snapshot(repo_root, &refreshed_config, &refreshed_harness)?;

    let updated_surfaces = preview
        .surfaces
        .iter()
        .map(|surface| AppliedHarnessSurface {
            key: surface.surface_key.clone(),
            path: surface.path.clone(),
            sha1: surface.after_sha1.clone(),
            bytes: surface.after_bytes,
        })
        .collect();

    Ok(HarnessApplyExecution {
        snapshot_before,
        snapshot_after,
        preview,
        updated_surfaces,
    })
}

fn normalize_edits(edits: &[HarnessSurfaceEdit]) -> Result<Vec<HarnessSurfaceEdit>> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for edit in edits {
        let surface_key = edit.surface_key.trim().to_string();
        ensure!(!surface_key.is_empty(), "surface_key is required");
        ensure!(
            supported_surface_path(&surface_key).is_some(),
            "unsupported surface_key: {}",
            surface_key
        );
        ensure!(
            seen.insert(surface_key.clone()),
            "duplicate surface_key: {}",
            surface_key
        );
        let content = edit.content.trim().to_string();
        ensure!(!content.is_empty(), "content for {} is empty", surface_key);
        normalized.push(HarnessSurfaceEdit {
            surface_key,
            content,
        });
    }
    Ok(normalized)
}

fn validate_surface_content(surface_key: &str, content: &str) -> Result<()> {
    ensure!(
        !content.trim().is_empty(),
        "content for {} must not be empty",
        surface_key
    );

    match surface_key {
        "tools.descriptions" => {
            let parsed = serde_json::from_str::<HashMap<String, String>>(content)
                .with_context(|| format!("{} must be a JSON object<string,string>", surface_key))?;
            ensure!(
                !parsed.is_empty(),
                "{} must contain at least one tool description",
                surface_key
            );
        }
        "middleware.messages" => {
            let parsed = serde_json::from_str::<serde_json::Value>(content)
                .with_context(|| format!("{} must be valid JSON", surface_key))?;
            ensure!(parsed.is_object(), "{} must be a JSON object", surface_key);
        }
        _ => {}
    }

    Ok(())
}

fn sha1_hex(content: &str) -> String {
    format!("{:x}", Sha1::digest(content.as_bytes()))
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HarnessSurfaceEdit, apply_harness_edits, preview_harness_edits, supported_surface_path,
    };
    use crate::config::model::AppConfig;
    use crate::domain::harness::HarnessAssets;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-happly-{}-{}",
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
    fn knows_supported_surface_paths() {
        assert_eq!(
            supported_surface_path("system.base"),
            Some("harness/system/base.md")
        );
        assert!(supported_surface_path("runtime.policy.self_evolution").is_none());
    }

    #[test]
    fn previews_supported_surface_edits_with_diff_metadata() {
        let repo = TestRepo::new();
        let config = AppConfig::default();
        let harness = HarnessAssets::load(&repo.root).unwrap();

        let preview = preview_harness_edits(
            &repo.root,
            &config,
            &harness,
            &[HarnessSurfaceEdit {
                surface_key: "system.base".to_string(),
                content: "line one\nline two".to_string(),
            }],
            Some(
                &crate::domain::harness::snapshot::current_harness_snapshot_id(
                    &repo.root, &config, &harness,
                )
                .unwrap(),
            ),
        )
        .unwrap();

        assert_eq!(preview.changed_surface_count, 1);
        assert_eq!(preview.surfaces.len(), 1);
        assert_eq!(preview.surfaces[0].surface_key, "system.base");
        assert!(preview.surfaces[0].changed);
        assert_eq!(preview.surfaces[0].after_lines, 2);
        assert_eq!(preview.surfaces[0].path, "harness/system/base.md");
    }

    #[test]
    fn applies_supported_surface_edits_and_updates_snapshot() {
        let repo = TestRepo::new();
        let config = AppConfig::default();
        let harness = HarnessAssets::load(&repo.root).unwrap();
        let result = apply_harness_edits(
            &repo.root,
            &config,
            &harness,
            &[
                HarnessSurfaceEdit {
                    surface_key: "system.base".to_string(),
                    content: "custom base".to_string(),
                },
                HarnessSurfaceEdit {
                    surface_key: "tools.descriptions".to_string(),
                    content: r#"{ "read_file": "Read carefully" }"#.to_string(),
                },
            ],
            Some(
                &crate::domain::harness::snapshot::current_harness_snapshot_id(
                    &repo.root, &config, &harness,
                )
                .unwrap(),
            ),
        )
        .unwrap();

        assert_ne!(
            result.snapshot_before.snapshot_id,
            result.snapshot_after.snapshot_id
        );
        assert_eq!(result.preview.changed_surface_count, 2);
        assert_eq!(result.updated_surfaces.len(), 2);
        assert_eq!(
            fs::read_to_string(repo.root.join("harness/system/base.md")).unwrap(),
            "custom base"
        );
        assert!(
            result
                .snapshot_after
                .surfaces
                .iter()
                .any(|surface| surface.key == "tools.descriptions"
                    && surface.content.contains("Read carefully"))
        );
    }
}
