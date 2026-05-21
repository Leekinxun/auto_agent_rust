use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::trace::HarnessRunTrace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessSignalSummary {
    pub inspected_traces: usize,
    pub memory_traces: usize,
    pub stateless_traces: usize,
    pub success_traces: usize,
    pub error_traces: usize,
    pub final_reply_recovered_traces: usize,
    pub max_iterations_traces: usize,
    pub self_evolution_executed_traces: usize,
    pub avg_iterations: f32,
    pub avg_tool_calls: f32,
    pub top_tools: Vec<HarnessToolSignal>,
    pub candidate_signals: Vec<HarnessCandidateSignal>,
    pub recent_trace_ids: Vec<String>,
    pub memory_only_self_evolution: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessToolSignal {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessCandidateSignal {
    pub key: String,
    pub severity: String,
    pub summary: String,
    pub trace_ids: Vec<String>,
}

pub fn summarize_harness_traces(traces: &[HarnessRunTrace]) -> HarnessSignalSummary {
    let inspected_traces = traces.len();
    let memory_traces = traces.iter().filter(|trace| trace.mode == "memory").count();
    let stateless_traces = traces.len().saturating_sub(memory_traces);
    let success_traces = traces
        .iter()
        .filter(|trace| trace.outcome.status == "success")
        .count();
    let error_traces = traces.len().saturating_sub(success_traces);
    let final_reply_recovered_traces = traces
        .iter()
        .filter(|trace| trace.outcome.final_reply_recovered)
        .count();
    let max_iterations_traces = traces
        .iter()
        .filter(|trace| trace.outcome.finish_reason == "max_iterations")
        .count();
    let self_evolution_executed_traces = traces
        .iter()
        .filter(|trace| trace.outcome.self_evolution_executed)
        .count();
    let avg_iterations = average(
        traces
            .iter()
            .map(|trace| trace.outcome.iterations)
            .collect::<Vec<_>>(),
    );
    let avg_tool_calls = average(
        traces
            .iter()
            .map(|trace| trace.outcome.tool_calls)
            .collect::<Vec<_>>(),
    );

    let mut tool_counts = HashMap::<String, usize>::new();
    for trace in traces {
        for tool_name in &trace.outcome.tool_names {
            *tool_counts.entry(tool_name.clone()).or_default() += 1;
        }
    }
    let mut top_tools = tool_counts
        .into_iter()
        .map(|(name, count)| HarnessToolSignal { name, count })
        .collect::<Vec<_>>();
    top_tools.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    top_tools.truncate(10);

    let candidate_signals = build_candidate_signals(traces, avg_tool_calls);

    HarnessSignalSummary {
        inspected_traces,
        memory_traces,
        stateless_traces,
        success_traces,
        error_traces,
        final_reply_recovered_traces,
        max_iterations_traces,
        self_evolution_executed_traces,
        avg_iterations,
        avg_tool_calls,
        top_tools,
        candidate_signals,
        recent_trace_ids: traces.iter().map(|trace| trace.trace_id.clone()).collect(),
        memory_only_self_evolution: true,
    }
}

fn build_candidate_signals(
    traces: &[HarnessRunTrace],
    avg_tool_calls: f32,
) -> Vec<HarnessCandidateSignal> {
    let mut signals = Vec::new();

    let max_iteration_ids = matching_trace_ids(traces, |trace| {
        trace.outcome.finish_reason == "max_iterations"
    });
    if !max_iteration_ids.is_empty() {
        signals.push(HarnessCandidateSignal {
            key: "max_iterations".to_string(),
            severity: severity_for_count(max_iteration_ids.len(), 1, 3),
            summary: format!(
                "{} recent runs exhausted max_iterations; review top-level/system and tool guidance for dead loops.",
                max_iteration_ids.len()
            ),
            trace_ids: max_iteration_ids,
        });
    }

    let recovered_ids = matching_trace_ids(traces, |trace| trace.outcome.final_reply_recovered);
    if !recovered_ids.is_empty() {
        signals.push(HarnessCandidateSignal {
            key: "final_reply_recovery".to_string(),
            severity: severity_for_count(recovered_ids.len(), 1, 3),
            summary: format!(
                "{} recent runs needed final-answer recovery; inspect recovery prompt and tool-description clarity.",
                recovered_ids.len()
            ),
            trace_ids: recovered_ids,
        });
    }

    let error_ids = matching_trace_ids(traces, |trace| trace.outcome.status == "error");
    if !error_ids.is_empty() {
        signals.push(HarnessCandidateSignal {
            key: "run_errors".to_string(),
            severity: severity_for_count(error_ids.len(), 1, 2),
            summary: format!(
                "{} recent runs ended in error; inspect failing tools or brittle instructions before changing prompts.",
                error_ids.len()
            ),
            trace_ids: error_ids,
        });
    }

    if avg_tool_calls >= 4.0 {
        let churn_ids = matching_trace_ids(traces, |trace| trace.outcome.tool_calls >= 4);
        signals.push(HarnessCandidateSignal {
            key: "tool_churn".to_string(),
            severity: if avg_tool_calls >= 7.0 {
                "high".to_string()
            } else {
                "medium".to_string()
            },
            summary: format!(
                "Average tool calls per run is {:.2}; consider tightening tool descriptions or subagent guidance to reduce churn.",
                avg_tool_calls
            ),
            trace_ids: churn_ids,
        });
    }

    let policy_violation_ids = matching_trace_ids(traces, |trace| {
        trace.mode == "stateless" && trace.outcome.self_evolution_executed
    });
    if !policy_violation_ids.is_empty() {
        signals.push(HarnessCandidateSignal {
            key: "stateless_policy_violation".to_string(),
            severity: "high".to_string(),
            summary:
                "Stateless traces show self-evolution execution; this violates the hard memory-only policy."
                    .to_string(),
            trace_ids: policy_violation_ids,
        });
    }

    signals
}

fn matching_trace_ids<F>(traces: &[HarnessRunTrace], predicate: F) -> Vec<String>
where
    F: Fn(&HarnessRunTrace) -> bool,
{
    traces
        .iter()
        .filter(|trace| predicate(trace))
        .map(|trace| trace.trace_id.clone())
        .collect()
}

fn severity_for_count(count: usize, medium_threshold: usize, high_threshold: usize) -> String {
    if count >= high_threshold {
        "high".to_string()
    } else if count >= medium_threshold {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn average(values: Vec<usize>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<usize>() as f32 / values.len() as f32
}

#[cfg(test)]
mod tests {
    use super::summarize_harness_traces;
    use crate::domain::harness::PromptSource;
    use crate::domain::harness::trace::{
        HarnessRunTrace, HarnessTraceOutcome, HarnessTracePrompts, HarnessTraceRequest,
    };

    fn trace(
        trace_id: &str,
        mode: &str,
        status: &str,
        finish_reason: &str,
        tool_names: &[&str],
        final_reply_recovered: bool,
        self_evolution_executed: bool,
    ) -> HarnessRunTrace {
        HarnessRunTrace {
            trace_id: trace_id.to_string(),
            harness_snapshot_id: "hsnap-demo".to_string(),
            started_at_ms: 1,
            finished_at_ms: 2,
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
                tool_names: tool_names.iter().map(|item| item.to_string()).collect(),
                reply_chars: 42,
                output_files: 0,
                output_file_names: Vec::new(),
                used_skill_names: Vec::new(),
                skills_updated: 0,
                final_reply_recovered,
                self_evolution_executed,
            },
        }
    }

    #[test]
    fn summarizes_trace_signals_and_preserves_memory_only_policy() {
        let summary = summarize_harness_traces(&[
            trace(
                "trace-3",
                "memory",
                "success",
                "max_iterations",
                &["read_file", "read_file", "task", "TodoWrite"],
                true,
                true,
            ),
            trace(
                "trace-2",
                "stateless",
                "error",
                "tool_error",
                &["read_file", "task", "task", "task"],
                false,
                false,
            ),
            trace(
                "trace-1",
                "stateless",
                "success",
                "stop",
                &["read_file", "task", "search_files", "read_file"],
                false,
                false,
            ),
        ]);

        assert_eq!(summary.inspected_traces, 3);
        assert_eq!(summary.memory_traces, 1);
        assert_eq!(summary.stateless_traces, 2);
        assert_eq!(summary.error_traces, 1);
        assert_eq!(summary.final_reply_recovered_traces, 1);
        assert_eq!(summary.max_iterations_traces, 1);
        assert_eq!(summary.self_evolution_executed_traces, 1);
        assert_eq!(summary.top_tools[0].name, "read_file");
        assert_eq!(summary.top_tools[0].count, 5);
        assert!(summary.avg_tool_calls >= 4.0);
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
        assert!(
            summary
                .candidate_signals
                .iter()
                .any(|signal| signal.key == "tool_churn")
        );
        assert_eq!(
            summary.recent_trace_ids,
            vec!["trace-3", "trace-2", "trace-1"]
        );
    }
}
