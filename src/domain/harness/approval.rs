use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::apply::HarnessPreviewSurface;

const APPROVAL_DIR: &str = ".omx/approvals/harness";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessApprovalStatus {
    Approved,
    Reverted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessApprovalChange {
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
pub struct HarnessApprovalRecord {
    pub approval_id: String,
    pub created_at_ms: u128,
    pub decision_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub approved_by: String,
    pub approval_note: Option<String>,
    pub mode_scope: String,
    pub related_trace_ids: Vec<String>,
    pub snapshot_before_id: String,
    pub snapshot_after_id: String,
    pub changed_surfaces: Vec<HarnessApprovalChange>,
    pub runtime_reloaded: bool,
    pub status: HarnessApprovalStatus,
    pub reverted_from_approval_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateHarnessApprovalInput {
    pub decision_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub approved_by: String,
    pub approval_note: Option<String>,
    pub mode_scope: Option<String>,
    pub related_trace_ids: Vec<String>,
    pub snapshot_before_id: String,
    pub snapshot_after_id: String,
    pub changed_surfaces: Vec<HarnessPreviewSurface>,
    pub runtime_reloaded: bool,
    pub status: HarnessApprovalStatus,
    pub reverted_from_approval_id: Option<String>,
}

impl HarnessApprovalRecord {
    pub fn create(input: CreateHarnessApprovalInput) -> Result<Self> {
        let title = required_text("title", input.title)?;
        let summary = required_text("summary", input.summary)?;
        let approved_by = required_text("approved_by", input.approved_by)?;
        let snapshot_before_id = required_text("snapshot_before_id", input.snapshot_before_id)?;
        let snapshot_after_id = required_text("snapshot_after_id", input.snapshot_after_id)?;
        let changed_surfaces = input
            .changed_surfaces
            .into_iter()
            .map(to_approval_change)
            .collect::<Vec<_>>();
        ensure!(
            !changed_surfaces.is_empty(),
            "at least one changed surface is required for approval"
        );

        Ok(Self {
            approval_id: new_approval_id(),
            created_at_ms: now_ms(),
            decision_id: optional_text(input.decision_id),
            title,
            summary,
            approved_by,
            approval_note: optional_text(input.approval_note),
            mode_scope: normalize_mode_scope(input.mode_scope)?,
            related_trace_ids: normalize_list(input.related_trace_ids),
            snapshot_before_id,
            snapshot_after_id,
            changed_surfaces,
            runtime_reloaded: input.runtime_reloaded,
            status: input.status,
            reverted_from_approval_id: optional_text(input.reverted_from_approval_id),
        })
    }
}

pub fn new_approval_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("happroval-{nanos}")
}

pub async fn write_harness_approval(
    repo_root: &Path,
    approval: &HarnessApprovalRecord,
) -> Result<PathBuf> {
    let approval_dir = repo_root.join(APPROVAL_DIR);
    tokio::fs::create_dir_all(&approval_dir)
        .await
        .with_context(|| format!("failed to create {}", approval_dir.display()))?;
    let path = approval_dir.join(format!("{}.json", approval.approval_id));
    let encoded =
        serde_json::to_string_pretty(approval).context("failed to encode harness approval json")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub async fn read_harness_approval(
    repo_root: &Path,
    approval_id: &str,
) -> Result<Option<HarnessApprovalRecord>> {
    let path = repo_root
        .join(APPROVAL_DIR)
        .join(format!("{}.json", approval_id));
    if !path.exists() {
        return Ok(None);
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(
        serde_json::from_str::<HarnessApprovalRecord>(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?,
    ))
}

pub async fn list_recent_harness_approvals(
    repo_root: &Path,
    limit: usize,
) -> Result<Vec<HarnessApprovalRecord>> {
    let dir = repo_root.join(APPROVAL_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = tokio::fs::read_dir(&dir)
        .await
        .with_context(|| format!("failed to read {}", dir.display()))?;
    let mut paths = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to iterate {}", dir.display()))?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.push(path);
        }
    }

    paths.sort();
    paths.reverse();

    let mut approvals = Vec::new();
    for path in paths.into_iter().take(limit.clamp(1, 200)) {
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let approval = serde_json::from_str::<HarnessApprovalRecord>(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        approvals.push(approval);
    }

    Ok(approvals)
}

fn to_approval_change(surface: HarnessPreviewSurface) -> HarnessApprovalChange {
    HarnessApprovalChange {
        surface_key: surface.surface_key,
        path: surface.path,
        changed: surface.changed,
        before_sha1: surface.before_sha1,
        after_sha1: surface.after_sha1,
        before_bytes: surface.before_bytes,
        after_bytes: surface.after_bytes,
        byte_delta: surface.byte_delta,
        before_lines: surface.before_lines,
        after_lines: surface.after_lines,
        line_delta: surface.line_delta,
        before_content: surface.before_content,
        after_content: surface.after_content,
    }
}

fn required_text(field: &str, value: String) -> Result<String> {
    let value = value.trim().to_string();
    ensure!(!value.is_empty(), "{field} is required");
    Ok(value)
}

fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_list(values: Vec<String>) -> Vec<String> {
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

fn normalize_mode_scope(value: Option<String>) -> Result<String> {
    let value = value
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .unwrap_or_else(|| "memory_only".to_string());
    ensure!(
        matches!(value.as_str(), "memory_only" | "all_modes"),
        "mode_scope must be one of: memory_only, all_modes"
    );
    Ok(value)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        CreateHarnessApprovalInput, HarnessApprovalStatus, list_recent_harness_approvals,
        read_harness_approval, write_harness_approval,
    };
    use crate::domain::harness::apply::HarnessPreviewSurface;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-happroval-{}-{}",
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

    fn sample_surface() -> HarnessPreviewSurface {
        HarnessPreviewSurface {
            surface_key: "system.base".to_string(),
            path: "harness/system/base.md".to_string(),
            changed: true,
            before_sha1: "before".to_string(),
            after_sha1: "after".to_string(),
            before_bytes: 4,
            after_bytes: 5,
            byte_delta: 1,
            before_lines: 1,
            after_lines: 2,
            line_delta: 1,
            before_content: "old".to_string(),
            after_content: "new\nvalue".to_string(),
        }
    }

    #[tokio::test]
    async fn creates_reads_and_lists_harness_approvals() {
        let repo = TestRepo::new();
        let approval = super::HarnessApprovalRecord::create(CreateHarnessApprovalInput {
            decision_id: Some("decision-1".to_string()),
            title: "Approve harness refinement".to_string(),
            summary: "Operator approved a reviewed prompt update".to_string(),
            approved_by: "alice".to_string(),
            approval_note: Some("Reviewed in UI".to_string()),
            mode_scope: None,
            related_trace_ids: vec!["trace-1".to_string()],
            snapshot_before_id: "hsnap-before".to_string(),
            snapshot_after_id: "hsnap-after".to_string(),
            changed_surfaces: vec![sample_surface()],
            runtime_reloaded: true,
            status: HarnessApprovalStatus::Approved,
            reverted_from_approval_id: None,
        })
        .unwrap();

        write_harness_approval(&repo.root, &approval).await.unwrap();
        let loaded = read_harness_approval(&repo.root, &approval.approval_id)
            .await
            .unwrap()
            .unwrap();
        let approvals = list_recent_harness_approvals(&repo.root, 10).await.unwrap();

        assert_eq!(approvals.len(), 1);
        assert_eq!(loaded.approved_by, "alice");
        assert_eq!(loaded.status, HarnessApprovalStatus::Approved);
        assert_eq!(loaded.changed_surfaces[0].before_content, "old");
        assert_eq!(loaded.changed_surfaces[0].surface_key, "system.base");
    }
}
