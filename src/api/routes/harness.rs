use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::api::errors::{ApiError, ApiResult};
use crate::app_state::SharedState;
use crate::config::loader::load_config;
use crate::domain::harness::HarnessAssets;
use crate::domain::harness::apply::{
    AppliedHarnessSurface, HarnessApplyPreview, HarnessSurfaceEdit, apply_harness_edits,
    preview_harness_edits,
};
use crate::domain::harness::approval::{
    CreateHarnessApprovalInput, HarnessApprovalRecord, HarnessApprovalStatus,
    list_recent_harness_approvals, read_harness_approval, write_harness_approval,
};
use crate::domain::harness::decision::{
    CreateHarnessDecisionInput, HarnessDecisionRecord, HarnessDecisionStatus,
    list_recent_harness_decisions, read_harness_decision, write_harness_decision,
};
use crate::domain::harness::draft::{HarnessDecisionDraft, generate_harness_decision_drafts};
use crate::domain::harness::signals::{HarnessSignalSummary, summarize_harness_traces};
use crate::domain::harness::snapshot::{HarnessSnapshot, build_harness_snapshot};
use crate::domain::harness::trace::{HarnessRunTrace, list_recent_harness_run_traces};

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/harness/snapshot", get(get_harness_snapshot))
        .route("/harness/signals", get(get_harness_signals))
        .route("/harness/drafts", get(list_harness_drafts))
        .route("/harness/traces", get(list_harness_traces))
        .route(
            "/harness/preview-apply",
            axum::routing::post(preview_harness_changes),
        )
        .route("/harness/apply", axum::routing::post(apply_harness_changes))
        .route("/harness/approvals", get(list_harness_approvals))
        .route(
            "/harness/rollback",
            axum::routing::post(rollback_harness_approval),
        )
        .route(
            "/harness/decisions",
            get(list_harness_decisions).post(create_harness_decision),
        )
}

#[derive(Debug, Deserialize)]
struct HarnessListQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CreateHarnessDecisionRequest {
    title: String,
    summary: String,
    rationale: String,
    #[serde(default)]
    expected_impact: Vec<String>,
    #[serde(default)]
    changed_surfaces: Vec<String>,
    #[serde(default)]
    validation_plan: Vec<String>,
    mode_scope: Option<String>,
    status: Option<HarnessDecisionStatus>,
    #[serde(default)]
    related_trace_ids: Vec<String>,
    snapshot_before_id: Option<String>,
    snapshot_after_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreviewHarnessRequest {
    expected_snapshot_id: Option<String>,
    #[serde(default)]
    edits: Vec<HarnessSurfaceEdit>,
}

#[derive(Debug, Deserialize)]
struct RollbackHarnessRequest {
    approval_id: String,
    approved_by: String,
    approval_note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApplyHarnessRequest {
    decision_id: Option<String>,
    expected_snapshot_id: Option<String>,
    title: String,
    summary: String,
    rationale: String,
    #[serde(default)]
    expected_impact: Vec<String>,
    #[serde(default)]
    changed_surfaces: Vec<String>,
    #[serde(default)]
    validation_plan: Vec<String>,
    mode_scope: Option<String>,
    #[serde(default)]
    related_trace_ids: Vec<String>,
    snapshot_before_id: Option<String>,
    approved_by: String,
    approval_note: Option<String>,
    #[serde(default)]
    edits: Vec<HarnessSurfaceEdit>,
}

#[derive(Debug, serde::Serialize)]
struct HarnessApplyResponse {
    decision: HarnessDecisionRecord,
    approval: HarnessApprovalRecord,
    snapshot_after: HarnessSnapshot,
    updated_surfaces: Vec<AppliedHarnessSurface>,
    runtime_reloaded: bool,
}

fn load_effective_harness_state(
    repo_root: &std::path::Path,
    fallback_config: &crate::config::model::AppConfig,
) -> Result<(crate::config::model::AppConfig, HarnessAssets), anyhow::Error> {
    let config_path = repo_root.join("config").join("config.yaml");
    let config = if config_path.exists() {
        load_config(repo_root)?
    } else {
        fallback_config.clone()
    };
    let harness = HarnessAssets::load(repo_root, &config)?;
    Ok((config, harness))
}

async fn get_harness_snapshot(
    State(state): State<SharedState>,
) -> ApiResult<Json<HarnessSnapshot>> {
    let (config, harness) = load_effective_harness_state(&state.repo_root, &state.config)?;
    Ok(Json(build_harness_snapshot(
        &state.repo_root,
        &config,
        &harness,
    )?))
}

async fn list_harness_traces(
    State(state): State<SharedState>,
    Query(query): Query<HarnessListQuery>,
) -> ApiResult<Json<Vec<HarnessRunTrace>>> {
    let limit = query.limit.unwrap_or(20);
    Ok(Json(
        list_recent_harness_run_traces(&state.repo_root, limit).await?,
    ))
}

async fn get_harness_signals(
    State(state): State<SharedState>,
    Query(query): Query<HarnessListQuery>,
) -> ApiResult<Json<HarnessSignalSummary>> {
    let limit = query.limit.unwrap_or(20);
    let traces = list_recent_harness_run_traces(&state.repo_root, limit).await?;
    Ok(Json(summarize_harness_traces(&traces)))
}

async fn list_harness_drafts(
    State(state): State<SharedState>,
    Query(query): Query<HarnessListQuery>,
) -> ApiResult<Json<Vec<HarnessDecisionDraft>>> {
    let limit = query.limit.unwrap_or(20);
    let traces = list_recent_harness_run_traces(&state.repo_root, limit).await?;
    let summary = summarize_harness_traces(&traces);
    let (config, harness) = load_effective_harness_state(&state.repo_root, &state.config)?;
    let snapshot = build_harness_snapshot(&state.repo_root, &config, &harness)?;
    Ok(Json(generate_harness_decision_drafts(&summary, &snapshot)?))
}

async fn list_harness_decisions(
    State(state): State<SharedState>,
    Query(query): Query<HarnessListQuery>,
) -> ApiResult<Json<Vec<HarnessDecisionRecord>>> {
    let limit = query.limit.unwrap_or(20);
    Ok(Json(
        list_recent_harness_decisions(&state.repo_root, limit).await?,
    ))
}

async fn list_harness_approvals(
    State(state): State<SharedState>,
    Query(query): Query<HarnessListQuery>,
) -> ApiResult<Json<Vec<HarnessApprovalRecord>>> {
    let limit = query.limit.unwrap_or(20);
    Ok(Json(
        list_recent_harness_approvals(&state.repo_root, limit).await?,
    ))
}

async fn create_harness_decision(
    State(state): State<SharedState>,
    Json(payload): Json<CreateHarnessDecisionRequest>,
) -> ApiResult<Json<HarnessDecisionRecord>> {
    let decision = HarnessDecisionRecord::create(CreateHarnessDecisionInput {
        title: payload.title,
        summary: payload.summary,
        rationale: payload.rationale,
        expected_impact: payload.expected_impact,
        changed_surfaces: payload.changed_surfaces,
        validation_plan: payload.validation_plan,
        mode_scope: payload.mode_scope,
        status: payload.status,
        related_trace_ids: payload.related_trace_ids,
        snapshot_before_id: payload.snapshot_before_id,
        snapshot_after_id: payload.snapshot_after_id,
    })
    .map_err(|error| ApiError::bad_request(error.to_string()))?;

    write_harness_decision(&state.repo_root, &decision).await?;
    Ok(Json(decision))
}

async fn preview_harness_changes(
    State(state): State<SharedState>,
    Json(payload): Json<PreviewHarnessRequest>,
) -> ApiResult<Json<HarnessApplyPreview>> {
    let (config, harness) = load_effective_harness_state(&state.repo_root, &state.config)?;
    let preview = preview_harness_edits(
        &state.repo_root,
        &config,
        &harness,
        &payload.edits,
        payload.expected_snapshot_id.as_deref(),
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    Ok(Json(preview))
}

async fn rollback_harness_approval(
    State(state): State<SharedState>,
    Json(payload): Json<RollbackHarnessRequest>,
) -> ApiResult<Json<HarnessApplyResponse>> {
    let approval = read_harness_approval(&state.repo_root, &payload.approval_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!("approval not found: {}", payload.approval_id))
        })?;

    let edits = approval
        .changed_surfaces
        .iter()
        .map(|surface| HarnessSurfaceEdit {
            surface_key: surface.surface_key.clone(),
            content: surface.before_content.clone(),
        })
        .collect::<Vec<_>>();

    let (config, harness) = load_effective_harness_state(&state.repo_root, &state.config)?;
    let execution = apply_harness_edits(
        &state.repo_root,
        &config,
        &harness,
        &edits,
        Some(&approval.snapshot_after_id),
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))?;

    let decision = HarnessDecisionRecord::create(CreateHarnessDecisionInput {
        title: format!("Rollback {}", approval.title),
        summary: format!(
            "Revert harness surfaces back to snapshot {}",
            approval.snapshot_before_id
        ),
        rationale: format!("Manual rollback from approval {}", approval.approval_id),
        expected_impact: vec!["restore previous harness behavior".to_string()],
        changed_surfaces: approval
            .changed_surfaces
            .iter()
            .map(|surface| surface.surface_key.clone())
            .collect(),
        validation_plan: vec!["review traces after rollback".to_string()],
        mode_scope: Some(approval.mode_scope.clone()),
        status: Some(HarnessDecisionStatus::Accepted),
        related_trace_ids: approval.related_trace_ids.clone(),
        snapshot_before_id: Some(execution.snapshot_before.snapshot_id.clone()),
        snapshot_after_id: Some(execution.snapshot_after.snapshot_id.clone()),
    })
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    write_harness_decision(&state.repo_root, &decision).await?;
    state.reload_chat_orchestrator()?;

    let rollback_approval = HarnessApprovalRecord::create(CreateHarnessApprovalInput {
        decision_id: Some(decision.decision_id.clone()),
        title: decision.title.clone(),
        summary: decision.summary.clone(),
        approved_by: payload.approved_by,
        approval_note: payload.approval_note,
        mode_scope: Some(decision.mode_scope.clone()),
        related_trace_ids: decision.related_trace_ids.clone(),
        snapshot_before_id: execution.snapshot_before.snapshot_id.clone(),
        snapshot_after_id: execution.snapshot_after.snapshot_id.clone(),
        changed_surfaces: execution.preview.surfaces.clone(),
        runtime_reloaded: true,
        status: HarnessApprovalStatus::Reverted,
        reverted_from_approval_id: Some(approval.approval_id.clone()),
    })
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    write_harness_approval(&state.repo_root, &rollback_approval).await?;

    Ok(Json(HarnessApplyResponse {
        decision,
        approval: rollback_approval,
        snapshot_after: execution.snapshot_after,
        updated_surfaces: execution.updated_surfaces,
        runtime_reloaded: true,
    }))
}

async fn apply_harness_changes(
    State(state): State<SharedState>,
    Json(payload): Json<ApplyHarnessRequest>,
) -> ApiResult<Json<HarnessApplyResponse>> {
    let (config, harness) = load_effective_harness_state(&state.repo_root, &state.config)?;
    let execution = apply_harness_edits(
        &state.repo_root,
        &config,
        &harness,
        &payload.edits,
        payload.expected_snapshot_id.as_deref(),
    )
    .map_err(|error| ApiError::bad_request(error.to_string()))?;

    let mut decision = if let Some(decision_id) = payload.decision_id.as_deref() {
        let mut existing = read_harness_decision(&state.repo_root, decision_id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("decision not found: {decision_id}")))?;
        existing.status = HarnessDecisionStatus::Accepted;
        if existing.snapshot_before_id.is_none() {
            existing.snapshot_before_id = Some(execution.snapshot_before.snapshot_id.clone());
        }
        existing.snapshot_after_id = Some(execution.snapshot_after.snapshot_id.clone());
        existing
    } else {
        HarnessDecisionRecord::create(CreateHarnessDecisionInput {
            title: payload.title,
            summary: payload.summary,
            rationale: payload.rationale,
            expected_impact: payload.expected_impact,
            changed_surfaces: payload.changed_surfaces,
            validation_plan: payload.validation_plan,
            mode_scope: payload.mode_scope,
            status: Some(HarnessDecisionStatus::Accepted),
            related_trace_ids: payload.related_trace_ids,
            snapshot_before_id: payload
                .snapshot_before_id
                .or_else(|| Some(execution.snapshot_before.snapshot_id.clone())),
            snapshot_after_id: Some(execution.snapshot_after.snapshot_id.clone()),
        })
        .map_err(|error| ApiError::bad_request(error.to_string()))?
    };

    if payload.decision_id.is_some() {
        decision.snapshot_after_id = Some(execution.snapshot_after.snapshot_id.clone());
    }

    write_harness_decision(&state.repo_root, &decision).await?;
    state.reload_chat_orchestrator()?;

    let approval = HarnessApprovalRecord::create(CreateHarnessApprovalInput {
        decision_id: Some(decision.decision_id.clone()),
        title: decision.title.clone(),
        summary: decision.summary.clone(),
        approved_by: payload.approved_by,
        approval_note: payload.approval_note,
        mode_scope: Some(decision.mode_scope.clone()),
        related_trace_ids: decision.related_trace_ids.clone(),
        snapshot_before_id: execution.snapshot_before.snapshot_id.clone(),
        snapshot_after_id: execution.snapshot_after.snapshot_id.clone(),
        changed_surfaces: execution.preview.surfaces.clone(),
        runtime_reloaded: true,
        status: HarnessApprovalStatus::Approved,
        reverted_from_approval_id: None,
    })
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    write_harness_approval(&state.repo_root, &approval).await?;

    Ok(Json(HarnessApplyResponse {
        decision,
        approval,
        snapshot_after: execution.snapshot_after,
        updated_surfaces: execution.updated_surfaces,
        runtime_reloaded: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyHarnessRequest, CreateHarnessDecisionRequest, HarnessListQuery, PreviewHarnessRequest,
        RollbackHarnessRequest, apply_harness_changes, create_harness_decision,
        get_harness_signals, get_harness_snapshot, list_harness_approvals, list_harness_decisions,
        list_harness_drafts, list_harness_traces, preview_harness_changes,
        rollback_harness_approval,
    };
    use crate::app_state::AppState;
    use crate::config::model::AppConfig;
    use crate::domain::harness::PromptSource;
    use crate::domain::harness::apply::HarnessSurfaceEdit;
    use crate::domain::harness::approval::HarnessApprovalStatus;
    use crate::domain::harness::decision::HarnessDecisionStatus;
    use crate::domain::harness::trace::{
        HarnessRunTrace, HarnessTraceOutcome, HarnessTracePrompts, HarnessTraceRequest, now_ms,
        write_harness_run_trace,
    };
    use axum::Json;
    use axum::extract::{Query, State};
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
                "auto-claude-harness-route-{}-{}-{}",
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

    fn build_test_state(repo_root: PathBuf, config: AppConfig) -> crate::app_state::SharedState {
        AppState::new(repo_root, config).unwrap()
    }

    #[tokio::test]
    async fn returns_current_harness_snapshot() {
        let repo = TestRepo::new();
        let mut config = AppConfig::default();
        config.agent.system_prompt = "snapshot base".to_string();

        let Json(snapshot) =
            get_harness_snapshot(State(build_test_state(repo.root.clone(), config)))
                .await
                .unwrap();

        assert!(snapshot.memory_only_self_evolution);
        assert!(snapshot.snapshot_id.starts_with("hsnap-"));
        assert_eq!(
            snapshot
                .surfaces
                .iter()
                .find(|surface| surface.key == "system.base")
                .unwrap()
                .content,
            "snapshot base"
        );
    }

    #[tokio::test]
    async fn returns_recent_harness_traces() {
        let repo = TestRepo::new();
        let trace = HarnessRunTrace {
            trace_id: "route-trace".to_string(),
            harness_snapshot_id: "hsnap-route".to_string(),
            started_at_ms: now_ms(),
            finished_at_ms: now_ms(),
            run_kind: "sync".to_string(),
            mode: "stateless".to_string(),
            request: HarnessTraceRequest {
                session_id: Some("session-1".to_string()),
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

        let Json(traces) = list_harness_traces(
            State(build_test_state(repo.root.clone(), AppConfig::default())),
            Query(HarnessListQuery { limit: Some(1) }),
        )
        .await
        .unwrap();

        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].trace_id, "route-trace");
        assert_eq!(traces[0].harness_snapshot_id, "hsnap-route");
    }

    #[tokio::test]
    async fn returns_aggregated_harness_signals() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());

        for (trace_id, mode, status, finish_reason, tool_names, recovered, evolved) in [
            (
                "trace-2",
                "memory",
                "success",
                "max_iterations",
                vec![
                    "read_file".to_string(),
                    "task".to_string(),
                    "task".to_string(),
                    "TodoWrite".to_string(),
                ],
                true,
                true,
            ),
            (
                "trace-1",
                "stateless",
                "error",
                "tool_error",
                vec![
                    "read_file".to_string(),
                    "search_files".to_string(),
                    "task".to_string(),
                    "task".to_string(),
                ],
                false,
                false,
            ),
        ] {
            let trace = HarnessRunTrace {
                trace_id: trace_id.to_string(),
                harness_snapshot_id: "hsnap-route".to_string(),
                started_at_ms: now_ms(),
                finished_at_ms: now_ms(),
                run_kind: "sync".to_string(),
                mode: mode.to_string(),
                request: HarnessTraceRequest {
                    session_id: None,
                    user_id_present: false,
                    history_items: 0,
                    uploaded_files: 0,
                    memory_snapshot_injected: false,
                    self_evolution_allowed: mode == "memory",
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
                    status: status.to_string(),
                    finish_reason: finish_reason.to_string(),
                    error: None,
                    iterations: 3,
                    tool_calls: tool_names.len(),
                    tool_names,
                    reply_chars: 0,
                    output_files: 0,
                    output_file_names: Vec::new(),
                    used_skill_names: Vec::new(),
                    skills_updated: 0,
                    final_reply_recovered: recovered,
                    self_evolution_executed: evolved,
                },
            };
            write_harness_run_trace(&repo.root, &trace).await.unwrap();
        }

        let Json(summary) =
            get_harness_signals(State(state), Query(HarnessListQuery { limit: Some(10) }))
                .await
                .unwrap();

        assert_eq!(summary.inspected_traces, 2);
        assert_eq!(summary.memory_traces, 1);
        assert_eq!(summary.stateless_traces, 1);
        assert_eq!(summary.max_iterations_traces, 1);
        assert_eq!(summary.error_traces, 1);
        assert!(summary.memory_only_self_evolution);
        assert!(
            summary
                .candidate_signals
                .iter()
                .any(|signal| signal.key == "max_iterations")
        );
        assert!(
            summary
                .candidate_signals
                .iter()
                .any(|signal| signal.key == "run_errors")
        );
    }

    #[tokio::test]
    async fn returns_generated_harness_drafts() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());

        let trace = HarnessRunTrace {
            trace_id: "trace-draft".to_string(),
            harness_snapshot_id: "hsnap-route".to_string(),
            started_at_ms: now_ms(),
            finished_at_ms: now_ms(),
            run_kind: "sync".to_string(),
            mode: "memory".to_string(),
            request: HarnessTraceRequest {
                session_id: None,
                user_id_present: false,
                history_items: 0,
                uploaded_files: 0,
                memory_snapshot_injected: false,
                self_evolution_allowed: true,
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
                finish_reason: "max_iterations".to_string(),
                error: None,
                iterations: 4,
                tool_calls: 5,
                tool_names: vec![
                    "read_file".to_string(),
                    "read_file".to_string(),
                    "task".to_string(),
                    "task".to_string(),
                    "search_files".to_string(),
                ],
                reply_chars: 0,
                output_files: 0,
                output_file_names: Vec::new(),
                used_skill_names: Vec::new(),
                skills_updated: 0,
                final_reply_recovered: true,
                self_evolution_executed: true,
            },
        };
        write_harness_run_trace(&repo.root, &trace).await.unwrap();

        let Json(drafts) =
            list_harness_drafts(State(state), Query(HarnessListQuery { limit: Some(10) }))
                .await
                .unwrap();

        assert!(!drafts.is_empty());
        assert!(drafts[0].draft_id.starts_with("hdraft-"));
        assert_eq!(drafts[0].mode_scope, "memory_only");
        assert!(drafts[0].snapshot_before_id.is_some());
    }

    #[tokio::test]
    async fn previews_harness_changes_before_apply() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());
        let initial_snapshot = get_harness_snapshot(State(state.clone())).await.unwrap().0;

        let Json(preview) = preview_harness_changes(
            State(state),
            Json(PreviewHarnessRequest {
                expected_snapshot_id: Some(initial_snapshot.snapshot_id.clone()),
                edits: vec![HarnessSurfaceEdit {
                    surface_key: "system.base".to_string(),
                    content: "manual preview content\nwith line two".to_string(),
                }],
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            preview.snapshot_before.snapshot_id,
            initial_snapshot.snapshot_id
        );
        assert_eq!(preview.changed_surface_count, 1);
        assert_eq!(
            preview.surfaces[0].before_content,
            initial_snapshot.surfaces[0].content
        );
        assert_eq!(preview.surfaces[0].after_lines, 2);
    }

    #[tokio::test]
    async fn applies_harness_changes_and_persists_accepted_decision_and_approval() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());
        let initial_snapshot = get_harness_snapshot(State(state.clone())).await.unwrap().0;

        let Json(result) = apply_harness_changes(
            State(state.clone()),
            Json(ApplyHarnessRequest {
                decision_id: None,
                expected_snapshot_id: Some(initial_snapshot.snapshot_id.clone()),
                title: "Apply manual harness refinement".to_string(),
                summary: "Tighten system guidance manually".to_string(),
                rationale: "Operator reviewed traces and confirmed the change".to_string(),
                expected_impact: vec!["fewer loops".to_string()],
                changed_surfaces: vec!["system.base".to_string()],
                validation_plan: vec!["review next traces".to_string()],
                mode_scope: Some("memory_only".to_string()),
                related_trace_ids: vec!["trace-1".to_string()],
                snapshot_before_id: Some(initial_snapshot.snapshot_id.clone()),
                approved_by: "operator-a".to_string(),
                approval_note: Some("approved from UI preview".to_string()),
                edits: vec![HarnessSurfaceEdit {
                    surface_key: "system.base".to_string(),
                    content: "manual apply content".to_string(),
                }],
            }),
        )
        .await
        .unwrap();

        assert_eq!(result.decision.status, HarnessDecisionStatus::Accepted);
        assert!(result.runtime_reloaded);
        assert_eq!(result.updated_surfaces.len(), 1);
        assert_eq!(result.approval.approved_by, "operator-a");
        assert_eq!(result.approval.status, HarnessApprovalStatus::Approved);
        assert_eq!(
            crate::config::loader::load_config(&repo.root)
                .unwrap()
                .agent
                .system_prompt,
            "manual apply content"
        );
        assert_ne!(
            result.decision.snapshot_before_id.as_deref(),
            result.decision.snapshot_after_id.as_deref()
        );

        let Json(approvals) =
            list_harness_approvals(State(state), Query(HarnessListQuery { limit: Some(10) }))
                .await
                .unwrap();
        assert_eq!(approvals.len(), 1);
        assert_eq!(
            approvals[0].decision_id.as_deref(),
            Some(result.decision.decision_id.as_str())
        );
    }

    #[tokio::test]
    async fn rolls_back_harness_changes_from_approval() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());
        let initial_snapshot = get_harness_snapshot(State(state.clone())).await.unwrap().0;

        let Json(applied) = apply_harness_changes(
            State(state.clone()),
            Json(ApplyHarnessRequest {
                decision_id: None,
                expected_snapshot_id: Some(initial_snapshot.snapshot_id.clone()),
                title: "Apply manual harness refinement".to_string(),
                summary: "Tighten system guidance manually".to_string(),
                rationale: "Operator reviewed traces and confirmed the change".to_string(),
                expected_impact: vec!["fewer loops".to_string()],
                changed_surfaces: vec!["system.base".to_string()],
                validation_plan: vec!["review next traces".to_string()],
                mode_scope: Some("memory_only".to_string()),
                related_trace_ids: vec!["trace-1".to_string()],
                snapshot_before_id: Some(initial_snapshot.snapshot_id.clone()),
                approved_by: "operator-a".to_string(),
                approval_note: Some("approved from UI preview".to_string()),
                edits: vec![HarnessSurfaceEdit {
                    surface_key: "system.base".to_string(),
                    content: "manual apply content".to_string(),
                }],
            }),
        )
        .await
        .unwrap();

        let Json(rolled_back) = rollback_harness_approval(
            State(state.clone()),
            Json(RollbackHarnessRequest {
                approval_id: applied.approval.approval_id.clone(),
                approved_by: "operator-b".to_string(),
                approval_note: Some("restore previous prompt".to_string()),
            }),
        )
        .await
        .unwrap();

        assert_eq!(rolled_back.approval.status, HarnessApprovalStatus::Reverted);
        assert_eq!(
            rolled_back.approval.reverted_from_approval_id.as_deref(),
            Some(applied.approval.approval_id.as_str())
        );
        assert_eq!(
            crate::config::loader::load_config(&repo.root)
                .unwrap()
                .agent
                .system_prompt,
            applied.approval.changed_surfaces[0].before_content
        );
    }

    #[tokio::test]
    async fn creates_and_lists_harness_decisions() {
        let repo = TestRepo::new();
        let state = build_test_state(repo.root.clone(), AppConfig::default());

        let Json(created) = create_harness_decision(
            State(state.clone()),
            Json(CreateHarnessDecisionRequest {
                title: "Refine harness tool guidance".to_string(),
                summary: "Tighten descriptions for noisy tools".to_string(),
                rationale: "Observed redundant tool loops in recent traces".to_string(),
                expected_impact: vec!["fewer redundant tool calls".to_string()],
                changed_surfaces: vec!["tools.descriptions".to_string()],
                validation_plan: vec!["review next 5 traces".to_string()],
                mode_scope: None,
                status: Some(HarnessDecisionStatus::Proposed),
                related_trace_ids: vec!["trace-1".to_string()],
                snapshot_before_id: Some("hsnap-before".to_string()),
                snapshot_after_id: None,
            }),
        )
        .await
        .unwrap();

        let Json(decisions) =
            list_harness_decisions(State(state), Query(HarnessListQuery { limit: Some(10) }))
                .await
                .unwrap();

        assert!(created.decision_id.starts_with("hdecision-"));
        assert_eq!(created.mode_scope, "memory_only");
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].title, "Refine harness tool guidance");
    }
}
