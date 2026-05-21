use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::decision::HarnessDecisionStatus;
use super::signals::{HarnessCandidateSignal, HarnessSignalSummary};
use super::snapshot::HarnessSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessDecisionDraft {
    pub draft_id: String,
    pub signal_key: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub rationale: String,
    pub expected_impact: Vec<String>,
    pub changed_surfaces: Vec<String>,
    pub validation_plan: Vec<String>,
    pub mode_scope: String,
    pub recommended_status: HarnessDecisionStatus,
    pub related_trace_ids: Vec<String>,
    pub snapshot_before_id: Option<String>,
}

pub fn generate_harness_decision_drafts(
    summary: &HarnessSignalSummary,
    snapshot: &HarnessSnapshot,
) -> Result<Vec<HarnessDecisionDraft>> {
    summary
        .candidate_signals
        .iter()
        .map(|signal| generate_draft(signal, snapshot, summary.memory_only_self_evolution))
        .collect()
}

fn generate_draft(
    signal: &HarnessCandidateSignal,
    snapshot: &HarnessSnapshot,
    memory_only_self_evolution: bool,
) -> Result<HarnessDecisionDraft> {
    let (title, rationale, expected_impact, changed_surfaces, validation_plan) =
        match signal.key.as_str() {
            "max_iterations" => (
                "Reduce repeated max-iteration stalls".to_string(),
                "Recent traces repeatedly exhausted the iteration budget, which suggests the current top-level guidance or tool descriptions allow loops without converging on a user-visible answer.".to_string(),
                vec![
                    "Lower the number of runs that terminate at max_iterations".to_string(),
                    "Reduce unproductive tool loops before the final answer".to_string(),
                ],
                vec![
                    "system.base".to_string(),
                    "subagents.shared".to_string(),
                    "subagents.general_purpose".to_string(),
                    "tools.descriptions".to_string(),
                ],
                vec![
                    "Review the related traces for repeated tool-call patterns".to_string(),
                    "Apply a minimal prompt/tool-description change in memory mode only".to_string(),
                    "Compare the next 5-10 traces for max_iterations frequency and tool-call count".to_string(),
                ],
            ),
            "final_reply_recovery" => (
                "Reduce final-answer recovery dependence".to_string(),
                "Several recent runs only produced a user-visible answer after the recovery middleware intervened, which suggests the harness is not reliably guiding the model to close the loop on its own.".to_string(),
                vec![
                    "Increase first-pass completion rate without recovery".to_string(),
                    "Make tool-use completion criteria clearer to the model".to_string(),
                ],
                vec![
                    "middleware.final_answer_recovery".to_string(),
                    "subagents.general_purpose".to_string(),
                    "tools.descriptions".to_string(),
                ],
                vec![
                    "Inspect recovered traces for missing final-answer patterns".to_string(),
                    "Adjust completion guidance conservatively".to_string(),
                    "Verify that subsequent runs end with direct final answers more often".to_string(),
                ],
            ),
            "run_errors" => (
                "Reduce harness-driven run errors".to_string(),
                "Recent runs failed before completing, so the next improvement should prioritize clearer tool expectations and safer orchestration before any aggressive self-evolution work.".to_string(),
                vec![
                    "Lower error-rate in recent harness traces".to_string(),
                    "Reduce brittle or ambiguous tool invocation patterns".to_string(),
                ],
                vec![
                    "tools.descriptions".to_string(),
                    "subagents.shared".to_string(),
                ],
                vec![
                    "Classify the related errors by tool and failure mode".to_string(),
                    "Only change harness guidance if the error is plausibly prompt-induced".to_string(),
                    "Re-check the next traces for error-rate changes".to_string(),
                ],
            ),
            "tool_churn" => (
                "Reduce unnecessary tool churn".to_string(),
                "The average tool-call count is elevated, which usually means the model lacks crisp guidance about when to stop searching, when to summarize, or which tool is most appropriate.".to_string(),
                vec![
                    "Reduce average tool calls per run".to_string(),
                    "Shorten time-to-answer for repetitive workflows".to_string(),
                ],
                vec![
                    "tools.descriptions".to_string(),
                    "subagents.shared".to_string(),
                    "subagents.explore".to_string(),
                    "subagents.general_purpose".to_string(),
                ],
                vec![
                    "Inspect top tools in recent traces to find avoidable repeats".to_string(),
                    "Tighten only the highest-churn guidance first".to_string(),
                    "Verify average tool calls and completion rate on the next batch of traces".to_string(),
                ],
            ),
            "stateless_policy_violation" => (
                "Reinforce stateless no-evolution guardrails".to_string(),
                "A stateless trace appears to have executed self-evolution behavior, which violates the hard policy that only memory mode may evolve.".to_string(),
                vec![
                    "Restore strict stateless no-evolution behavior".to_string(),
                    "Prevent future prompt/runtime drift from bypassing the policy".to_string(),
                ],
                vec!["runtime.policy.self_evolution".to_string()],
                vec![
                    "Audit the related traces and execution path immediately".to_string(),
                    "Verify that stateless traces always report self_evolution_executed=false".to_string(),
                    "Keep any mitigation active before resuming prompt experiments".to_string(),
                ],
            ),
            _ => (
                format!("Investigate harness signal: {}", signal.key),
                signal.summary.clone(),
                vec!["Capture a narrower, evidence-backed improvement target".to_string()],
                Vec::new(),
                vec![
                    "Review the related traces".to_string(),
                    "Draft the smallest safe harness change".to_string(),
                    "Validate the next traces before broadening scope".to_string(),
                ],
            ),
        };

    let manifest = serde_json::to_string(&serde_json::json!({
        "signal_key": signal.key,
        "severity": signal.severity,
        "trace_ids": signal.trace_ids,
        "snapshot_before_id": snapshot.snapshot_id,
    }))
    .context("failed to encode harness draft manifest")?;

    Ok(HarnessDecisionDraft {
        draft_id: format!("hdraft-{}", sha1_hex(&manifest)),
        signal_key: signal.key.clone(),
        severity: signal.severity.clone(),
        title,
        summary: signal.summary.clone(),
        rationale,
        expected_impact,
        changed_surfaces,
        validation_plan,
        mode_scope: if memory_only_self_evolution {
            "memory_only".to_string()
        } else {
            "all_modes".to_string()
        },
        recommended_status: HarnessDecisionStatus::Proposed,
        related_trace_ids: signal.trace_ids.clone(),
        snapshot_before_id: Some(snapshot.snapshot_id.clone()),
    })
}

fn sha1_hex(content: &str) -> String {
    format!("{:x}", Sha1::digest(content.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::generate_harness_decision_drafts;
    use crate::domain::harness::signals::{HarnessCandidateSignal, HarnessSignalSummary};
    use crate::domain::harness::snapshot::{HarnessSnapshot, HarnessSnapshotSurface};
    use crate::domain::harness::{PromptSource, PromptSourceKind};

    #[test]
    fn generates_memory_only_drafts_from_candidate_signals() {
        let summary = HarnessSignalSummary {
            inspected_traces: 2,
            memory_traces: 1,
            stateless_traces: 1,
            success_traces: 1,
            error_traces: 1,
            final_reply_recovered_traces: 1,
            max_iterations_traces: 1,
            self_evolution_executed_traces: 1,
            avg_iterations: 3.0,
            avg_tool_calls: 4.5,
            top_tools: Vec::new(),
            candidate_signals: vec![HarnessCandidateSignal {
                key: "tool_churn".to_string(),
                severity: "medium".to_string(),
                summary: "Average tool calls per run is elevated".to_string(),
                trace_ids: vec!["trace-1".to_string(), "trace-2".to_string()],
            }],
            recent_trace_ids: vec!["trace-2".to_string(), "trace-1".to_string()],
            memory_only_self_evolution: true,
        };
        let snapshot = HarnessSnapshot {
            snapshot_id: "hsnap-demo".to_string(),
            generated_at_ms: 1,
            memory_only_self_evolution: true,
            surfaces: vec![HarnessSnapshotSurface {
                key: "system.base".to_string(),
                source: PromptSource {
                    kind: PromptSourceKind::Builtin,
                    path: None,
                },
                sha1: "abc".to_string(),
                bytes: 3,
                content: "xyz".to_string(),
            }],
        };

        let drafts = generate_harness_decision_drafts(&summary, &snapshot).unwrap();

        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].draft_id.starts_with("hdraft-"));
        assert_eq!(drafts[0].signal_key, "tool_churn");
        assert_eq!(drafts[0].mode_scope, "memory_only");
        assert_eq!(drafts[0].snapshot_before_id.as_deref(), Some("hsnap-demo"));
        assert!(
            drafts[0]
                .changed_surfaces
                .contains(&"tools.descriptions".to_string())
        );
    }
}
