use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::PromptSource;

const TRACE_DIR: &str = ".omx/traces/harness";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessRunTrace {
    pub trace_id: String,
    pub harness_snapshot_id: String,
    pub started_at_ms: u128,
    pub finished_at_ms: u128,
    pub run_kind: String,
    pub mode: String,
    pub request: HarnessTraceRequest,
    pub prompts: HarnessTracePrompts,
    pub outcome: HarnessTraceOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessTraceRequest {
    pub session_id: Option<String>,
    pub user_id_present: bool,
    pub history_items: usize,
    pub uploaded_files: usize,
    pub memory_snapshot_injected: bool,
    pub self_evolution_allowed: bool,
    pub resolved_model_id: String,
    pub resolved_max_iterations: usize,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub mcp_base_urls: usize,
    pub mcp_disabled_urls: usize,
    pub mcp_lazy_urls: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessTracePrompts {
    pub top_level_system: PromptSource,
    pub system_append: PromptSource,
    pub final_answer_recovery: PromptSource,
    pub subagent_shared: PromptSource,
    pub subagent_explore: PromptSource,
    pub subagent_general: PromptSource,
    pub memory_maintenance_system: PromptSource,
    pub memory_maintenance_user_template: PromptSource,
    pub skill_learning_system: PromptSource,
    pub skill_learning_user_template: PromptSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessTraceOutcome {
    pub status: String,
    pub finish_reason: String,
    pub error: Option<String>,
    pub iterations: usize,
    pub tool_calls: usize,
    pub tool_names: Vec<String>,
    pub reply_chars: usize,
    pub output_files: usize,
    pub output_file_names: Vec<String>,
    pub used_skill_names: Vec<String>,
    pub skills_updated: usize,
    pub final_reply_recovered: bool,
    pub self_evolution_executed: bool,
}

pub fn new_trace_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("htrace-{nanos}")
}

pub async fn write_harness_run_trace(repo_root: &Path, trace: &HarnessRunTrace) -> Result<PathBuf> {
    let trace_dir = repo_root.join(TRACE_DIR);
    tokio::fs::create_dir_all(&trace_dir)
        .await
        .with_context(|| format!("failed to create {}", trace_dir.display()))?;
    let path = trace_dir.join(format!("{}.json", trace.trace_id));
    let encoded =
        serde_json::to_string_pretty(trace).context("failed to encode harness trace json")?;
    tokio::fs::write(&path, encoded)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub async fn list_recent_harness_run_traces(
    repo_root: &Path,
    limit: usize,
) -> Result<Vec<HarnessRunTrace>> {
    let dir = repo_root.join(TRACE_DIR);
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

    let mut traces = Vec::new();
    for path in paths.into_iter().take(limit.clamp(1, 200)) {
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let trace = serde_json::from_str::<HarnessRunTrace>(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        traces.push(trace);
    }

    Ok(traces)
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        HarnessRunTrace, HarnessTraceOutcome, HarnessTracePrompts, HarnessTraceRequest,
        list_recent_harness_run_traces, now_ms, write_harness_run_trace,
    };
    use crate::domain::harness::PromptSource;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEST_REPO_ID: AtomicU64 = AtomicU64::new(1);

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique_id = NEXT_TEST_REPO_ID.fetch_add(1, Ordering::Relaxed);
            let unique = format!(
                "auto-claude-htrace-{}-{}-{}",
                std::process::id(),
                unique_id,
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
    async fn writes_trace_json_under_omx_traces_directory() {
        let repo = TestRepo::new();
        let trace = HarnessRunTrace {
            trace_id: "demo-trace".to_string(),
            harness_snapshot_id: "hsnap-demo".to_string(),
            started_at_ms: now_ms(),
            finished_at_ms: now_ms(),
            run_kind: "sync".to_string(),
            mode: "stateless".to_string(),
            request: HarnessTraceRequest {
                session_id: Some("session-a".to_string()),
                user_id_present: false,
                history_items: 1,
                uploaded_files: 0,
                memory_snapshot_injected: false,
                self_evolution_allowed: false,
                resolved_model_id: "demo-model".to_string(),
                resolved_max_iterations: 8,
                temperature: None,
                top_p: None,
                mcp_base_urls: 0,
                mcp_disabled_urls: 0,
                mcp_lazy_urls: 0,
            },
            prompts: HarnessTracePrompts {
                top_level_system: PromptSource::builtin(),
                system_append: PromptSource::none(),
                final_answer_recovery: PromptSource::file("harness/middleware/final.md"),
                subagent_shared: PromptSource::builtin(),
                subagent_explore: PromptSource::builtin(),
                subagent_general: PromptSource::builtin(),
                memory_maintenance_system: PromptSource::builtin(),
                memory_maintenance_user_template: PromptSource::builtin(),
                skill_learning_system: PromptSource::builtin(),
                skill_learning_user_template: PromptSource::builtin(),
            },
            outcome: HarnessTraceOutcome {
                status: "success".to_string(),
                finish_reason: "stop".to_string(),
                error: None,
                iterations: 1,
                tool_calls: 0,
                tool_names: Vec::new(),
                reply_chars: 12,
                output_files: 0,
                output_file_names: Vec::new(),
                used_skill_names: Vec::new(),
                skills_updated: 0,
                final_reply_recovered: false,
                self_evolution_executed: false,
            },
        };

        let path = write_harness_run_trace(&repo.root, &trace).await.unwrap();
        let content = tokio::fs::read_to_string(&path).await.unwrap();

        assert!(path.ends_with(".omx/traces/harness/demo-trace.json"));
        assert!(content.contains("\"mode\": \"stateless\""));
        assert!(content.contains("\"harness_snapshot_id\": \"hsnap-demo\""));
        assert!(content.contains("\"self_evolution_allowed\": false"));
        assert!(content.contains("\"status\": \"success\""));
    }

    #[tokio::test]
    async fn lists_recent_traces_in_reverse_filename_order() {
        let repo = TestRepo::new();
        for trace_id in ["trace-1", "trace-2"] {
            let trace = HarnessRunTrace {
                trace_id: trace_id.to_string(),
                harness_snapshot_id: "hsnap-demo".to_string(),
                started_at_ms: now_ms(),
                finished_at_ms: now_ms(),
                run_kind: "sync".to_string(),
                mode: "stateless".to_string(),
                request: HarnessTraceRequest {
                    session_id: None,
                    user_id_present: false,
                    history_items: 0,
                    uploaded_files: 0,
                    memory_snapshot_injected: false,
                    self_evolution_allowed: false,
                    resolved_model_id: "demo-model".to_string(),
                    resolved_max_iterations: 8,
                    temperature: None,
                    top_p: None,
                    mcp_base_urls: 0,
                    mcp_disabled_urls: 0,
                    mcp_lazy_urls: 0,
                },
                prompts: HarnessTracePrompts {
                    top_level_system: PromptSource::builtin(),
                    system_append: PromptSource::none(),
                    final_answer_recovery: PromptSource::builtin(),
                    subagent_shared: PromptSource::builtin(),
                    subagent_explore: PromptSource::builtin(),
                    subagent_general: PromptSource::builtin(),
                    memory_maintenance_system: PromptSource::builtin(),
                    memory_maintenance_user_template: PromptSource::builtin(),
                    skill_learning_system: PromptSource::builtin(),
                    skill_learning_user_template: PromptSource::builtin(),
                },
                outcome: HarnessTraceOutcome {
                    status: "success".to_string(),
                    finish_reason: "stop".to_string(),
                    error: None,
                    iterations: 1,
                    tool_calls: 0,
                    tool_names: Vec::new(),
                    reply_chars: 0,
                    output_files: 0,
                    output_file_names: Vec::new(),
                    used_skill_names: Vec::new(),
                    skills_updated: 0,
                    final_reply_recovered: false,
                    self_evolution_executed: false,
                },
            };
            write_harness_run_trace(&repo.root, &trace).await.unwrap();
        }

        let traces = list_recent_harness_run_traces(&repo.root, 10)
            .await
            .unwrap();
        let trace_ids = traces
            .into_iter()
            .map(|item| item.trace_id)
            .collect::<Vec<_>>();

        assert_eq!(trace_ids, vec!["trace-2", "trace-1"]);
    }
}
