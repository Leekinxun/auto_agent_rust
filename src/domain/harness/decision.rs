use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

const DECISION_DIR: &str = ".omx/decisions/harness";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessDecisionStatus {
    Proposed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessDecisionRecord {
    pub decision_id: String,
    pub created_at_ms: u128,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub expected_impact: Vec<String>,
    pub changed_surfaces: Vec<String>,
    pub validation_plan: Vec<String>,
    pub mode_scope: String,
    pub status: HarnessDecisionStatus,
    pub related_trace_ids: Vec<String>,
    pub snapshot_before_id: Option<String>,
    pub snapshot_after_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateHarnessDecisionInput {
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub expected_impact: Vec<String>,
    pub changed_surfaces: Vec<String>,
    pub validation_plan: Vec<String>,
    pub mode_scope: Option<String>,
    pub status: Option<HarnessDecisionStatus>,
    pub related_trace_ids: Vec<String>,
    pub snapshot_before_id: Option<String>,
    pub snapshot_after_id: Option<String>,
}

impl HarnessDecisionRecord {
    pub fn create(input: CreateHarnessDecisionInput) -> Result<Self> {
        let title = required_text("title", input.title)?;
        let summary = required_text("summary", input.summary)?;
        let rationale = required_text("rationale", input.rationale)?;
        let mode_scope = normalize_mode_scope(input.mode_scope)?;

        Ok(Self {
            decision_id: new_decision_id(),
            created_at_ms: now_ms(),
            title,
            summary,
            rationale,
            expected_impact: normalize_list(input.expected_impact),
            changed_surfaces: normalize_list(input.changed_surfaces),
            validation_plan: normalize_list(input.validation_plan),
            mode_scope,
            status: input.status.unwrap_or(HarnessDecisionStatus::Proposed),
            related_trace_ids: normalize_list(input.related_trace_ids),
            snapshot_before_id: optional_text(input.snapshot_before_id),
            snapshot_after_id: optional_text(input.snapshot_after_id),
        })
    }
}

pub fn new_decision_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("hdecision-{nanos}")
}

pub async fn write_harness_decision(
    repo_root: &Path,
    decision: &HarnessDecisionRecord,
) -> Result<PathBuf> {
    let decision_dir = repo_root.join(DECISION_DIR);
    tokio::fs::create_dir_all(&decision_dir)
        .await
        .with_context(|| format!("failed to create {}", decision_dir.display()))?;
    let path = decision_dir.join(format!("{}.json", decision.decision_id));
    let encoded =
        serde_json::to_string_pretty(decision).context("failed to encode harness decision json")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub async fn read_harness_decision(
    repo_root: &Path,
    decision_id: &str,
) -> Result<Option<HarnessDecisionRecord>> {
    let path = repo_root
        .join(DECISION_DIR)
        .join(format!("{}.json", decision_id));
    if !path.exists() {
        return Ok(None);
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(
        serde_json::from_str::<HarnessDecisionRecord>(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?,
    ))
}

pub async fn list_recent_harness_decisions(
    repo_root: &Path,
    limit: usize,
) -> Result<Vec<HarnessDecisionRecord>> {
    let dir = repo_root.join(DECISION_DIR);
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

    let mut decisions = Vec::new();
    for path in paths.into_iter().take(limit.clamp(1, 200)) {
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let decision = serde_json::from_str::<HarnessDecisionRecord>(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        decisions.push(decision);
    }

    Ok(decisions)
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
        CreateHarnessDecisionInput, HarnessDecisionRecord, HarnessDecisionStatus,
        list_recent_harness_decisions, read_harness_decision, write_harness_decision,
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
                "auto-claude-hdecision-{}-{}",
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
    fn creates_memory_only_decision_by_default() {
        let decision = HarnessDecisionRecord::create(CreateHarnessDecisionInput {
            title: " Improve harness ".to_string(),
            summary: " Tighten tool guidance ".to_string(),
            rationale: " Reduce noisy tool churn ".to_string(),
            expected_impact: vec![" fewer retries ".to_string(), "fewer retries".to_string()],
            changed_surfaces: vec!["tools.descriptions".to_string()],
            validation_plan: vec!["run trace review".to_string()],
            mode_scope: None,
            status: Some(HarnessDecisionStatus::Accepted),
            related_trace_ids: vec![" trace-1 ".to_string()],
            snapshot_before_id: Some(" before ".to_string()),
            snapshot_after_id: Some(" after ".to_string()),
        })
        .unwrap();

        assert_eq!(decision.mode_scope, "memory_only");
        assert_eq!(decision.status, HarnessDecisionStatus::Accepted);
        assert_eq!(decision.expected_impact, vec!["fewer retries"]);
        assert_eq!(decision.snapshot_before_id.as_deref(), Some("before"));
    }

    #[tokio::test]
    async fn writes_and_lists_recent_decisions() {
        let repo = TestRepo::new();
        for title in ["first", "second"] {
            let decision = HarnessDecisionRecord::create(CreateHarnessDecisionInput {
                title: title.to_string(),
                summary: format!("{title} summary"),
                rationale: format!("{title} rationale"),
                expected_impact: Vec::new(),
                changed_surfaces: Vec::new(),
                validation_plan: Vec::new(),
                mode_scope: None,
                status: None,
                related_trace_ids: Vec::new(),
                snapshot_before_id: None,
                snapshot_after_id: None,
            })
            .unwrap();
            write_harness_decision(&repo.root, &decision).await.unwrap();
        }

        let decisions = list_recent_harness_decisions(&repo.root, 10).await.unwrap();

        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].title, "second");
        assert_eq!(decisions[1].title, "first");
        assert_eq!(
            read_harness_decision(&repo.root, &decisions[0].decision_id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "second"
        );
    }
}
