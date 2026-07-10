use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::infra::llm::types::ChatMessage;
use crate::support::sanitize::safe_user_dir_name;

const RUNS_DIR: &str = ".omx/runs";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunManifest {
    pub run_id: String,
    pub trace_id: String,
    pub user_id: String,
    pub session_id: Option<String>,
    pub mode: String,
    pub run_kind: String,
    pub model_id: String,
    pub harness_snapshot_id: String,
    pub started_at_ms: u128,
    pub finished_at_ms: Option<u128>,
    pub status: String,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
    pub step_count: usize,
    pub last_step_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunStep {
    pub run_id: String,
    pub step_index: usize,
    pub kind: AgentRunStepKind,
    pub created_at_ms: u128,
    pub iteration: usize,
    pub messages_before: Vec<ChatMessage>,
    pub messages_after: Vec<ChatMessage>,
    pub token_estimate_before: usize,
    pub token_estimate_after: usize,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStepKind {
    InitialState,
    Compaction,
    LlmRequest,
    LlmResponse,
    ToolCall,
    ToolResult,
    Final,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunDetail {
    pub manifest: AgentRunManifest,
    pub steps: Vec<AgentRunStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunReplay {
    pub run_id: String,
    pub from_step_index: usize,
    pub manifest: AgentRunManifest,
    pub steps: Vec<AgentRunStep>,
    pub replay_mode: String,
}

#[derive(Debug, Clone)]
pub struct AgentRunRecorder {
    repo_root: PathBuf,
    user_id: String,
    run_id: String,
}

impl AgentRunRecorder {
    pub async fn start(
        repo_root: &Path,
        user_id: &str,
        trace_id: &str,
        session_id: Option<String>,
        mode: &str,
        run_kind: &str,
        model_id: &str,
        harness_snapshot_id: &str,
        started_at_ms: u128,
    ) -> Result<Self> {
        let run_id = format!("arun-{}", trace_id.trim_start_matches("htrace-"));
        let recorder = Self {
            repo_root: repo_root.to_path_buf(),
            user_id: user_id.to_string(),
            run_id,
        };
        tokio::fs::create_dir_all(recorder.steps_dir())
            .await
            .with_context(|| format!("failed to create {}", recorder.steps_dir().display()))?;
        recorder
            .write_manifest(&AgentRunManifest {
                run_id: recorder.run_id.clone(),
                trace_id: trace_id.to_string(),
                user_id: user_id.to_string(),
                session_id,
                mode: mode.to_string(),
                run_kind: run_kind.to_string(),
                model_id: model_id.to_string(),
                harness_snapshot_id: harness_snapshot_id.to_string(),
                started_at_ms,
                finished_at_ms: None,
                status: "running".to_string(),
                finish_reason: None,
                error: None,
                step_count: 0,
                last_step_index: None,
            })
            .await?;
        Ok(recorder)
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub async fn record_step(&self, step: AgentRunStep) -> Result<()> {
        let path = self
            .steps_dir()
            .join(format!("{:06}.json", step.step_index));
        let encoded =
            serde_json::to_string_pretty(&step).context("failed to encode agent run step")?;
        tokio::fs::write(&path, encoded)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        self.update_manifest_counts(Some(step.step_index)).await
    }

    pub async fn finish(
        &self,
        status: &str,
        finish_reason: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        let mut manifest = read_agent_run_manifest(&self.repo_root, &self.user_id, &self.run_id)
            .await?
            .context("agent run manifest missing during finish")?;
        manifest.finished_at_ms = Some(now_ms());
        manifest.status = status.to_string();
        manifest.finish_reason = finish_reason;
        manifest.error = error;
        self.write_manifest(&manifest).await
    }

    async fn update_manifest_counts(&self, last_step_index: Option<usize>) -> Result<()> {
        let mut manifest = read_agent_run_manifest(&self.repo_root, &self.user_id, &self.run_id)
            .await?
            .context("agent run manifest missing during step update")?;
        manifest.step_count = manifest.step_count.saturating_add(1);
        manifest.last_step_index = last_step_index;
        self.write_manifest(&manifest).await
    }

    async fn write_manifest(&self, manifest: &AgentRunManifest) -> Result<()> {
        let path = self.run_dir().join("manifest.json");
        let encoded =
            serde_json::to_string_pretty(manifest).context("failed to encode run manifest")?;
        tokio::fs::write(&path, encoded)
            .await
            .with_context(|| format!("failed to write {}", path.display()))
    }

    fn run_dir(&self) -> PathBuf {
        run_dir(&self.repo_root, &self.user_id, &self.run_id)
    }

    fn steps_dir(&self) -> PathBuf {
        self.run_dir().join("steps")
    }
}

pub fn build_run_step(
    run_id: &str,
    step_index: usize,
    kind: AgentRunStepKind,
    iteration: usize,
    messages_before: Vec<ChatMessage>,
    messages_after: Vec<ChatMessage>,
    payload: Value,
) -> AgentRunStep {
    AgentRunStep {
        run_id: run_id.to_string(),
        step_index,
        kind,
        created_at_ms: now_ms(),
        iteration,
        token_estimate_before: crate::domain::chat::compaction::estimate_tokens(&messages_before),
        token_estimate_after: crate::domain::chat::compaction::estimate_tokens(&messages_after),
        messages_before,
        messages_after,
        payload,
    }
}

pub async fn list_agent_run_manifests(
    repo_root: &Path,
    user_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AgentRunManifest>> {
    let mut manifests = Vec::new();
    let root = repo_root.join(RUNS_DIR);
    if !root.exists() {
        return Ok(manifests);
    }

    if let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) {
        read_user_manifests(repo_root, user_id, &mut manifests).await?;
    } else {
        let mut users = tokio::fs::read_dir(&root)
            .await
            .with_context(|| format!("failed to read {}", root.display()))?;
        while let Some(user) = users
            .next_entry()
            .await
            .with_context(|| format!("failed to iterate {}", root.display()))?
        {
            if user
                .file_type()
                .await
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
            {
                read_sanitized_user_manifests(user.path(), &mut manifests).await?;
            }
        }
    }

    manifests.sort_by(|left, right| right.started_at_ms.cmp(&left.started_at_ms));
    manifests.truncate(limit.clamp(1, 500));
    Ok(manifests)
}

pub async fn read_agent_run_detail(
    repo_root: &Path,
    user_id: &str,
    run_id: &str,
) -> Result<Option<AgentRunDetail>> {
    let Some(manifest) = read_agent_run_manifest(repo_root, user_id, run_id).await? else {
        return Ok(None);
    };
    let steps = read_agent_run_steps(repo_root, user_id, run_id).await?;
    Ok(Some(AgentRunDetail { manifest, steps }))
}

pub async fn replay_agent_run(
    repo_root: &Path,
    user_id: &str,
    run_id: &str,
    from_step_index: usize,
) -> Result<Option<AgentRunReplay>> {
    let Some(detail) = read_agent_run_detail(repo_root, user_id, run_id).await? else {
        return Ok(None);
    };
    let steps = detail
        .steps
        .into_iter()
        .filter(|step| step.step_index >= from_step_index)
        .collect::<Vec<_>>();
    Ok(Some(AgentRunReplay {
        run_id: run_id.to_string(),
        from_step_index,
        manifest: detail.manifest,
        steps,
        replay_mode: "offline_recorded".to_string(),
    }))
}

async fn read_user_manifests(
    repo_root: &Path,
    user_id: &str,
    manifests: &mut Vec<AgentRunManifest>,
) -> Result<()> {
    let user_dir = repo_root.join(RUNS_DIR).join(safe_user_dir_name(user_id));
    read_sanitized_user_manifests(user_dir, manifests).await
}

async fn read_sanitized_user_manifests(
    user_dir: PathBuf,
    manifests: &mut Vec<AgentRunManifest>,
) -> Result<()> {
    if !user_dir.exists() {
        return Ok(());
    }
    let mut runs = tokio::fs::read_dir(&user_dir)
        .await
        .with_context(|| format!("failed to read {}", user_dir.display()))?;
    while let Some(run) = runs
        .next_entry()
        .await
        .with_context(|| format!("failed to iterate {}", user_dir.display()))?
    {
        let path = run.path().join("manifest.json");
        if path.exists()
            && let Ok(content) = tokio::fs::read_to_string(&path).await
            && let Ok(manifest) = serde_json::from_str::<AgentRunManifest>(&content)
        {
            manifests.push(manifest);
        }
    }
    Ok(())
}

async fn read_agent_run_manifest(
    repo_root: &Path,
    user_id: &str,
    run_id: &str,
) -> Result<Option<AgentRunManifest>> {
    let path = run_dir(repo_root, user_id, run_id).join("manifest.json");
    if !path.exists() {
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(serde_json::from_str(&content).with_context(|| {
        format!("failed to parse {}", path.display())
    })?))
}

async fn read_agent_run_steps(
    repo_root: &Path,
    user_id: &str,
    run_id: &str,
) -> Result<Vec<AgentRunStep>> {
    let dir = run_dir(repo_root, user_id, run_id).join("steps");
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
    let mut steps = Vec::new();
    for path in paths {
        let content = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        steps.push(
            serde_json::from_str(&content)
                .with_context(|| format!("failed to parse {}", path.display()))?,
        );
    }
    Ok(steps)
}

fn run_dir(repo_root: &Path, user_id: &str, run_id: &str) -> PathBuf {
    repo_root
        .join(RUNS_DIR)
        .join(safe_user_dir_name(user_id))
        .join(run_id)
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
