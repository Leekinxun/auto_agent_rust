use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use regex::Regex;
use serde_json::json;
use tokio::sync::mpsc;

use crate::config::model::AppConfig;
use crate::domain::chat::compaction::{
    auto_compact, estimate_tokens, microcompact, public_compaction_tool_schemas,
};
use crate::domain::chat::models::{
    AgentPromptOverrides, AgentPromptSettingsPreview, ChatEvent, ChatMode, ChatRequest, ChatResult,
    HistoryEntry, HitlOverrides, McpServerPreviewDto, McpSettingsPreview, OutputFile,
    SkillPermissions, SkillUsage, SystemPromptPreview,
};
use crate::domain::chat::tool_context::{
    ToolContextBudget, enforce_tool_turn_budget, prepare_tool_result_for_context,
};
use crate::domain::harness::HarnessAssets;
use crate::domain::harness::snapshot::current_harness_snapshot_id;
use crate::domain::harness::trace::{
    HarnessRunTrace, HarnessTraceOutcome, HarnessTracePrompts, HarnessTraceRequest, new_trace_id,
    now_ms, write_harness_run_trace,
};
use crate::domain::harness::{
    MEMORY_MAINTENANCE_SYSTEM_PATH, MEMORY_MAINTENANCE_USER_TEMPLATE_PATH, PromptSource,
    SKILL_LEARNING_SYSTEM_PATH, SKILL_LEARNING_USER_TEMPLATE_PATH, resolve_repo_prompt_source,
};
use crate::domain::hitl::models::{
    HitlDecisionRequest, HitlDecisionResolution, HitlDecisionStatus,
};
use crate::domain::hitl::policy::{HitlPolicyDecision, evaluate_tool_hitl_policy};
use crate::domain::hitl::service::{write_hitl_request, write_hitl_resolution};
use crate::domain::memory::service::UserMemoryService;
use crate::domain::session::service::{SessionContext, SessionService};
use crate::domain::skills::models::{SkillDocument, SkillScope};
use crate::domain::skills::service::SkillService;
use crate::domain::tasks::service::{TaskService, public_task_tool_schemas};
use crate::domain::worktree::service::{WorktreeService, public_worktree_tool_schemas};
use crate::infra::fs::tool_ops::{
    dispatch_public_file_tool, public_file_tool_schemas, safe_workspace_path,
};
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{
    ChatCompletionRequest, ChatMessage, StreamChunk, ToolCall, ToolCallAccumulator,
};
use crate::infra::mcp::client::McpClient;
use crate::infra::mcp::client::McpToolSelection;

const PAST_CONTEXT_TAG: &str = "past_context";
const PAST_CONTEXT_ACK: &str = "Noted. I'll reference this context only if relevant.";
const SEARCH_LAZY_MCP_TOOLS_TOOL: &str = "search_lazy_mcp_tools";
const ACTIVATE_LAZY_MCP_TOOLS_TOOL: &str = "activate_lazy_mcp_tools";

#[derive(Clone)]
pub struct ChatOrchestrator {
    repo_root: PathBuf,
    config: AppConfig,
    harness: HarnessAssets,
    llm_client: LlmClient,
    mcp_client: McpClient,
    memory_service: UserMemoryService,
    skill_service: SkillService,
    task_service: TaskService,
    worktree_service: WorktreeService,
    session_service: SessionService,
}

impl ChatOrchestrator {
    pub fn new(
        repo_root: PathBuf,
        config: AppConfig,
        harness: HarnessAssets,
        llm_client: LlmClient,
        mcp_client: McpClient,
        memory_service: UserMemoryService,
        skill_service: SkillService,
        task_service: TaskService,
        worktree_service: WorktreeService,
        session_service: SessionService,
    ) -> Self {
        Self {
            repo_root,
            config,
            harness,
            llm_client,
            mcp_client,
            memory_service,
            skill_service,
            task_service,
            worktree_service,
            session_service,
        }
    }

    pub async fn run(&self, request: ChatRequest, mode: ChatMode) -> Result<ChatResult> {
        let mut skill_usages = Vec::new();
        log_agent_input(&mode, "sync", &request);
        let prepared = self.prepare_request(request, &mode)?;
        let max_iterations =
            resolve_max_iterations(&prepared.llm_overrides, self.config.agent.max_iterations);
        let mut messages = prepared.messages;
        let mut mcp_tool_selection = McpToolSelection::default();
        let mut reply = String::new();
        let mut finish_reason = "stop".to_string();
        let mut rounds_without_todo = 0usize;
        let mut iterations = 0usize;
        let mut tool_names = Vec::new();
        let mut final_reply_recovered = false;
        let active_run_generation = prepared
            .session
            .as_deref()
            .map(SessionContext::begin_agent_run);
        let started_at_ms = now_ms();
        let trace_id = new_trace_id();

        let run_result: Result<()> = async {
            for _ in 0..max_iterations {
                iterations += 1;
                log_agent_iteration_start(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "sync",
                    iterations,
                    messages.len(),
                );
                microcompact(&mut messages);
                if estimate_tokens(&messages) > self.config.agent.auto_compact_token_threshold {
                    tracing::info!(
                        mode = mode.as_str(),
                        run_kind = "sync",
                        iteration = iterations,
                        "agent context auto-compaction triggered"
                    );
                    messages = auto_compact(
                        &self.repo_root,
                        &self.config,
                        &self.llm_client,
                        messages.clone(),
                    )
                    .await?;
                }
                self.inject_session_messages(
                    &mut messages,
                    prepared.session.as_deref(),
                    active_run_generation,
                )
                .await?;
                let tools = self
                    .load_public_tools_with_overrides(
                        prepared.session.is_some(),
                        Some(&prepared.mcp_overrides),
                        &mcp_tool_selection,
                    )
                    .await;
                let request_body = self.build_llm_request_options(
                    messages.clone(),
                    Some(tools),
                    false,
                    &prepared.llm_overrides,
                )?;
                log_llm_request(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "sync",
                    iterations,
                    &request_body,
                );
                let response = self.llm_client.chat(&request_body).await?;
                let Some(choice) = response.choices.into_iter().next() else {
                    finish_reason = "empty".to_string();
                    break;
                };
                finish_reason = choice.finish_reason.unwrap_or_else(|| "stop".to_string());
                let assistant = choice.message;
                let tool_calls = assistant.tool_calls.clone();
                let assistant_content = assistant.content.clone().unwrap_or_default();
                log_llm_response(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "sync",
                    iterations,
                    &finish_reason,
                    &assistant_content,
                    &tool_calls,
                );
                messages.push(assistant.into_chat_message());
                if tool_calls.is_empty() {
                    append_reply_segment(&mut reply, &assistant_content);
                    if self.apply_steering_interrupt(
                        &mut messages,
                        prepared.session.as_deref(),
                        active_run_generation,
                        None,
                        None,
                    )? {
                        continue;
                    }
                    break;
                }

                let mut compress_requested = false;
                let mut steering_interrupted = false;
                let turn_tool_message_start = messages.len();
                for (index, tool_call) in tool_calls.iter().enumerate() {
                    tool_names.push(tool_call.function.name.clone());
                    log_tool_call(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "sync",
                        iterations,
                        index + 1,
                        tool_call,
                    );
                    let result = self
                        .dispatch_public_tool(
                            tool_call,
                            prepared.session.as_deref(),
                            prepared.user_id.as_deref(),
                            matches!(mode, ChatMode::Memory),
                            &prepared.mcp_overrides,
                            &mut mcp_tool_selection,
                            &mut skill_usages,
                            &prepared.skill_permissions,
                            &prepared.hitl_overrides,
                            active_run_generation,
                            None,
                        )
                        .await;
                    log_tool_result(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "sync",
                        iterations,
                        index + 1,
                        &tool_call.function.name,
                        &result,
                    );
                    if tool_call.function.name == "compress" {
                        compress_requested = true;
                    }
                    let context_result = self
                        .prepare_tool_result_for_context(tool_call, result)
                        .await;
                    log_tool_context_result(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "sync",
                        iterations,
                        index + 1,
                        &tool_call.function.name,
                        &context_result,
                    );
                    messages.push(ChatMessage::tool(
                        tool_call.id.clone(),
                        context_result.model_content,
                    ));

                    if self.apply_steering_interrupt(
                        &mut messages,
                        prepared.session.as_deref(),
                        active_run_generation,
                        Some(&tool_calls[index + 1..]),
                        None,
                    )? {
                        steering_interrupted = true;
                        break;
                    }
                }

                self.enforce_tool_turn_budget(&mut messages, turn_tool_message_start)
                    .await;

                if compress_requested {
                    messages = auto_compact(
                        &self.repo_root,
                        &self.config,
                        &self.llm_client,
                        messages.clone(),
                    )
                    .await?;
                }

                if steering_interrupted {
                    continue;
                }

                self.apply_todo_reminder(
                    &mut messages,
                    prepared.session.as_deref(),
                    &tool_calls,
                    &mut rounds_without_todo,
                );
            }
            Ok(())
        }
        .await;

        if let (Some(session), Some(generation)) =
            (prepared.session.as_deref(), active_run_generation)
        {
            session.end_agent_run(generation);
        }

        if let Err(error) = run_result {
            self.persist_harness_trace(build_harness_trace(
                &prepared.trace_request,
                &prepared.trace_prompts,
                &trace_id,
                &prepared.trace_snapshot_id,
                started_at_ms,
                "sync",
                mode.as_str(),
                "error".to_string(),
                finish_reason.clone(),
                Some(format!("{error:#}")),
                iterations,
                &tool_names,
                0,
                &[],
                &skill_usages,
                0,
                false,
                final_reply_recovered,
            ))
            .await;
            return Err(error);
        }

        if reply.trim().is_empty() {
            if let Some(recovered_reply) = self
                .recover_missing_final_reply(&messages, &prepared.llm_overrides, &finish_reason)
                .await?
            {
                reply = recovered_reply;
                finish_reason = "stop".to_string();
                final_reply_recovered = true;
            }
        }

        log_final_reply(
            &mode,
            prepared.session.as_deref(),
            prepared.user_id.as_deref(),
            &finish_reason,
            &reply,
        );

        let output_files = extract_output_files(&self.repo_root, &reply);
        let history =
            build_response_history(&prepared.cleaned_history, &prepared.user_message, &reply);
        let skills_updated = self
            .finalize_memory_side_effects(
                &mode,
                prepared.session.as_deref(),
                prepared.user_id.as_deref(),
                &prepared.user_message,
                &reply,
                &skill_usages,
                &prepared.prompt_overrides,
            )
            .await;
        self.persist_harness_trace(build_harness_trace(
            &prepared.trace_request,
            &prepared.trace_prompts,
            &trace_id,
            &prepared.trace_snapshot_id,
            started_at_ms,
            "sync",
            mode.as_str(),
            "success".to_string(),
            finish_reason.clone(),
            None,
            iterations,
            &tool_names,
            reply.chars().count(),
            &output_files,
            &skill_usages,
            skills_updated.len(),
            mode.allows_self_evolution(),
            final_reply_recovered,
        ))
        .await;

        Ok(ChatResult {
            reply,
            history,
            output_files,
            skills_updated,
        })
    }

    pub async fn stream(
        &self,
        request: ChatRequest,
        mode: ChatMode,
        sender: mpsc::Sender<ChatEvent>,
    ) -> Result<()> {
        let mut skill_usages = Vec::new();
        log_agent_input(&mode, "stream", &request);
        let prepared = self.prepare_request(request, &mode)?;
        let max_iterations =
            resolve_max_iterations(&prepared.llm_overrides, self.config.agent.max_iterations);
        let mut messages = prepared.messages;
        let mut mcp_tool_selection = McpToolSelection::default();
        let mut full_reply = String::new();
        let mut rounds_without_todo = 0usize;
        let mut iterations = 0usize;
        let mut tool_names = Vec::new();
        let mut final_reply_recovered = false;
        let mut final_finish_reason = "stop".to_string();
        let active_run_generation = prepared
            .session
            .as_deref()
            .map(SessionContext::begin_agent_run);
        let started_at_ms = now_ms();
        let trace_id = new_trace_id();

        let stream_result: Result<()> = async {
            for _ in 0..max_iterations {
                iterations += 1;
                log_agent_iteration_start(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "stream",
                    iterations,
                    messages.len(),
                );
                if sender.is_closed() {
                    tracing::info!("stream receiver closed before next iteration");
                    return Ok(());
                }
                microcompact(&mut messages);
                if estimate_tokens(&messages) > self.config.agent.auto_compact_token_threshold {
                    tracing::info!(
                        mode = mode.as_str(),
                        run_kind = "stream",
                        iteration = iterations,
                        "agent context auto-compaction triggered"
                    );
                    messages = auto_compact(
                        &self.repo_root,
                        &self.config,
                        &self.llm_client,
                        messages.clone(),
                    )
                    .await?;
                }
                self.inject_session_messages(
                    &mut messages,
                    prepared.session.as_deref(),
                    active_run_generation,
                )
                .await?;
                let tools = self
                    .load_public_tools_with_overrides(
                        prepared.session.is_some(),
                        Some(&prepared.mcp_overrides),
                        &mcp_tool_selection,
                    )
                    .await;
                let request_body = self.build_llm_request_options(
                    messages.clone(),
                    Some(tools),
                    true,
                    &prepared.llm_overrides,
                )?;
                log_llm_request(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "stream",
                    iterations,
                    &request_body,
                );
                let mut stream = self.llm_client.stream_chat(&request_body).await?;
                let mut buffer = Vec::new();
                let mut tool_accumulators: Vec<ToolCallAccumulator> = Vec::new();
                let mut round_text = String::new();
                let mut finish_reason = "stop".to_string();

                loop {
                    let chunk = tokio::select! {
                        _ = sender.closed() => {
                            tracing::info!("stream receiver closed during llm stream");
                            return Ok(());
                        }
                        chunk = stream.next() => chunk,
                    };
                    let Some(chunk) = chunk else {
                        break;
                    };
                    let bytes = chunk.context("failed to read llm stream chunk")?;
                    buffer.extend_from_slice(&bytes);

                    while let Some(frame) = next_sse_frame(&mut buffer)? {
                        process_llm_sse_frame(
                            &frame,
                            &mut tool_accumulators,
                            &mut round_text,
                            &mut full_reply,
                            &mut finish_reason,
                            &sender,
                        )
                        .await?;
                    }
                }

                if !buffer.is_empty() {
                    let frame = std::str::from_utf8(&buffer)
                        .context("llm stream returned invalid utf-8 in trailing frame")?
                        .to_string();
                    buffer.clear();
                    process_llm_sse_frame(
                        &frame,
                        &mut tool_accumulators,
                        &mut round_text,
                        &mut full_reply,
                        &mut finish_reason,
                        &sender,
                    )
                    .await?;
                }

                let tool_calls = tool_accumulators
                    .into_iter()
                    .filter(|item| !item.name.is_empty())
                    .map(ToolCallAccumulator::into_tool_call)
                    .collect::<Vec<_>>();
                log_llm_response(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    "stream",
                    iterations,
                    &finish_reason,
                    &round_text,
                    &tool_calls,
                );

                messages.push(ChatMessage::assistant(
                    if round_text.is_empty() {
                        None
                    } else {
                        Some(round_text)
                    },
                    tool_calls.clone(),
                ));

                if tool_calls.is_empty() {
                    if full_reply.trim().is_empty() {
                        if let Some(recovered_reply) = self
                            .recover_missing_final_reply(
                                &messages,
                                &prepared.llm_overrides,
                                &finish_reason,
                            )
                            .await?
                        {
                            full_reply = recovered_reply.clone();
                            finish_reason = "stop".to_string();
                            final_reply_recovered = true;
                            if !try_send_stream_event(&sender, ChatEvent::Text(recovered_reply))
                                .await
                            {
                                return Ok(());
                            }
                        } else {
                            let detail =
                                build_empty_stream_reply_error(&finish_reason, max_iterations);
                            tracing::warn!(finish_reason, "stream ended without visible reply");
                            let _ =
                                try_send_stream_event(&sender, ChatEvent::Error { detail }).await;
                            return Ok(());
                        }
                    }

                    if self.apply_steering_interrupt(
                        &mut messages,
                        prepared.session.as_deref(),
                        active_run_generation,
                        None,
                        Some(&sender),
                    )? {
                        continue;
                    }

                    let output_files = extract_output_files(&self.repo_root, &full_reply);
                    if !output_files.is_empty() {
                        if !try_send_stream_event(&sender, ChatEvent::OutputFiles(output_files))
                            .await
                        {
                            return Ok(());
                        }
                    }
                    self.spawn_stream_memory_side_effects(
                        &mode,
                        prepared.session.clone(),
                        prepared.user_id.clone(),
                        prepared.user_message.clone(),
                        full_reply.clone(),
                        skill_usages.clone(),
                        prepared.prompt_overrides.clone(),
                        prepared.trace_request.clone(),
                        prepared.trace_prompts.clone(),
                        trace_id.clone(),
                        prepared.trace_snapshot_id.clone(),
                        started_at_ms,
                        "stream".to_string(),
                        finish_reason.clone(),
                        iterations,
                        tool_names.clone(),
                        final_reply_recovered,
                    );
                    log_final_reply(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        &finish_reason,
                        &full_reply,
                    );
                    final_finish_reason = finish_reason.clone();
                    if !mode.allows_self_evolution() {
                        let output_files = extract_output_files(&self.repo_root, &full_reply);
                        self.persist_harness_trace(build_harness_trace(
                            &prepared.trace_request,
                            &prepared.trace_prompts,
                            &trace_id,
                            &prepared.trace_snapshot_id,
                            started_at_ms,
                            "stream",
                            mode.as_str(),
                            "success".to_string(),
                            finish_reason.clone(),
                            None,
                            iterations,
                            &tool_names,
                            full_reply.chars().count(),
                            &output_files,
                            &skill_usages,
                            0,
                            false,
                            final_reply_recovered,
                        ))
                        .await;
                    }
                    let _ = try_send_stream_event(&sender, ChatEvent::Done { finish_reason }).await;
                    return Ok(());
                }

                let mut compress_requested = false;
                let mut steering_interrupted = false;
                let turn_tool_message_start = messages.len();
                for (index, tool_call) in tool_calls.iter().enumerate() {
                    tool_names.push(tool_call.function.name.clone());
                    log_tool_call(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "stream",
                        iterations,
                        index + 1,
                        tool_call,
                    );
                    if !try_send_stream_event(
                        &sender,
                        ChatEvent::ToolUse {
                            name: tool_call.function.name.clone(),
                            arguments: tool_call.function.arguments.clone(),
                        },
                    )
                    .await
                    {
                        return Ok(());
                    }
                    let result = self
                        .dispatch_public_tool(
                            &tool_call,
                            prepared.session.as_deref(),
                            prepared.user_id.as_deref(),
                            matches!(mode, ChatMode::Memory),
                            &prepared.mcp_overrides,
                            &mut mcp_tool_selection,
                            &mut skill_usages,
                            &prepared.skill_permissions,
                            &prepared.hitl_overrides,
                            active_run_generation,
                            Some(&sender),
                        )
                        .await;
                    log_tool_result(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "stream",
                        iterations,
                        index + 1,
                        &tool_call.function.name,
                        &result,
                    );
                    if tool_call.function.name == "compress" {
                        compress_requested = true;
                    }
                    let preview = truncate_for_preview(&result, 2_000);
                    if !try_send_stream_event(
                        &sender,
                        ChatEvent::ToolResult {
                            tool: tool_call.function.name.clone(),
                            output: preview,
                        },
                    )
                    .await
                    {
                        return Ok(());
                    }
                    let context_result = self
                        .prepare_tool_result_for_context(tool_call, result)
                        .await;
                    log_tool_context_result(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        "stream",
                        iterations,
                        index + 1,
                        &tool_call.function.name,
                        &context_result,
                    );
                    messages.push(ChatMessage::tool(
                        tool_call.id.clone(),
                        context_result.model_content,
                    ));

                    if self.apply_steering_interrupt(
                        &mut messages,
                        prepared.session.as_deref(),
                        active_run_generation,
                        Some(&tool_calls[index + 1..]),
                        Some(&sender),
                    )? {
                        steering_interrupted = true;
                        break;
                    }
                }

                self.enforce_tool_turn_budget(&mut messages, turn_tool_message_start)
                    .await;

                if compress_requested {
                    messages = auto_compact(
                        &self.repo_root,
                        &self.config,
                        &self.llm_client,
                        messages.clone(),
                    )
                    .await?;
                }

                if steering_interrupted {
                    continue;
                }

                self.apply_todo_reminder(
                    &mut messages,
                    prepared.session.as_deref(),
                    &tool_calls,
                    &mut rounds_without_todo,
                );
            }

            let mut final_finish_reason = "max_iterations".to_string();
            if full_reply.trim().is_empty() {
                if let Some(recovered_reply) = self
                    .recover_missing_final_reply(
                        &messages,
                        &prepared.llm_overrides,
                        "max_iterations",
                    )
                    .await?
                {
                    full_reply = recovered_reply.clone();
                    final_finish_reason = "stop".to_string();
                    final_reply_recovered = true;
                    if !try_send_stream_event(&sender, ChatEvent::Text(recovered_reply)).await {
                        return Ok(());
                    }
                } else {
                    let detail = build_empty_stream_reply_error("max_iterations", max_iterations);
                    tracing::warn!(
                        max_iterations,
                        "stream exhausted iteration budget without visible reply"
                    );
                    let _ = try_send_stream_event(&sender, ChatEvent::Error { detail }).await;
                    return Ok(());
                }
            }

            if !try_send_stream_event(
                &sender,
                ChatEvent::Done {
                    finish_reason: final_finish_reason.clone(),
                },
            )
            .await
            {
                return Ok(());
            }
            self.spawn_stream_memory_side_effects(
                &mode,
                prepared.session.clone(),
                prepared.user_id.clone(),
                prepared.user_message.clone(),
                full_reply.clone(),
                skill_usages.clone(),
                prepared.prompt_overrides.clone(),
                prepared.trace_request.clone(),
                prepared.trace_prompts.clone(),
                trace_id.clone(),
                prepared.trace_snapshot_id.clone(),
                started_at_ms,
                "stream".to_string(),
                final_finish_reason.clone(),
                iterations,
                tool_names.clone(),
                final_reply_recovered,
            );
            log_final_reply(
                &mode,
                prepared.session.as_deref(),
                prepared.user_id.as_deref(),
                &final_finish_reason,
                &full_reply,
            );
            if !mode.allows_self_evolution() {
                let output_files = extract_output_files(&self.repo_root, &full_reply);
                self.persist_harness_trace(build_harness_trace(
                    &prepared.trace_request,
                    &prepared.trace_prompts,
                    &trace_id,
                    &prepared.trace_snapshot_id,
                    started_at_ms,
                    "stream",
                    mode.as_str(),
                    "success".to_string(),
                    final_finish_reason.clone(),
                    None,
                    iterations,
                    &tool_names,
                    full_reply.chars().count(),
                    &output_files,
                    &skill_usages,
                    0,
                    false,
                    final_reply_recovered,
                ))
                .await;
            }
            Ok(())
        }
        .await;

        if let (Some(session), Some(generation)) =
            (prepared.session.as_deref(), active_run_generation)
        {
            session.end_agent_run(generation);
        }

        if let Err(error) = stream_result {
            self.persist_harness_trace(build_harness_trace(
                &prepared.trace_request,
                &prepared.trace_prompts,
                &trace_id,
                &prepared.trace_snapshot_id,
                started_at_ms,
                "stream",
                mode.as_str(),
                "error".to_string(),
                final_finish_reason,
                Some(format!("{error:#}")),
                iterations,
                &tool_names,
                full_reply.chars().count(),
                &[],
                &skill_usages,
                0,
                false,
                final_reply_recovered,
            ))
            .await;
            return Err(error);
        }

        Ok(())
    }

    fn prepare_request(&self, request: ChatRequest, mode: &ChatMode) -> Result<PreparedRequest> {
        let resolved_max_iterations =
            max_iterations_from_request(&request.llm_overrides, self.config.agent.max_iterations);
        let trace_request =
            build_trace_request(&request, mode, &self.config, resolved_max_iterations);
        let trace_prompts = build_trace_prompts(&self.repo_root, &self.harness, &request, mode);
        let trace_snapshot_id =
            current_harness_snapshot_id(&self.repo_root, &self.config, &self.harness)?;
        let cleaned_history = if matches!(mode, ChatMode::Memory) {
            strip_cross_session_injections(request.history)
        } else {
            request.history
        };
        let user_message = append_uploaded_files(request.message, &request.files);
        let session = request
            .session_id
            .as_deref()
            .map(|session_id| self.session_service.get_or_create(session_id))
            .transpose()?;
        let mut base_system = request
            .system_override
            .or(request.system)
            .unwrap_or_else(|| {
                self.build_system(request.user_id.as_deref(), &request.skill_permissions)
            });
        base_system = append_system_instruction(base_system, request.system_append.as_deref());

        let system_prompt = if matches!(mode, ChatMode::Memory) {
            let snapshot = request
                .user_id
                .as_deref()
                .map(|user_id| self.load_memory_snapshot_for_prompt(session.as_deref(), user_id))
                .transpose()?;
            self.memory_service
                .build_user_memory_system(&base_system, snapshot.as_ref())
        } else {
            base_system
        };

        let mut messages = vec![ChatMessage::system(system_prompt)];
        for entry in &cleaned_history {
            messages.push(ChatMessage {
                role: entry.role.clone(),
                content: Some(entry.content.clone()),
                tool_call_id: None,
                tool_calls: None,
            });
        }
        messages.push(ChatMessage::user(user_message.clone()));

        Ok(PreparedRequest {
            messages,
            cleaned_history,
            user_message,
            session,
            user_id: request.user_id,
            llm_overrides: request.llm_overrides,
            prompt_overrides: request.prompt_overrides,
            mcp_overrides: request.mcp_overrides,
            skill_permissions: request.skill_permissions,
            hitl_overrides: request.hitl_overrides,
            trace_request,
            trace_prompts,
            trace_snapshot_id,
        })
    }

    pub fn preview_system_prompts(&self, user_id: Option<&str>) -> Result<SystemPromptPreview> {
        let normalized_user_id = user_id.map(str::trim).filter(|value| !value.is_empty());
        let default_permissions = SkillPermissions::default();
        let stateless_prompt = self.build_system(None, &default_permissions);
        let memory_base_prompt = self.build_system(normalized_user_id, &default_permissions);
        let memory_prompt = if let Some(user_id) = normalized_user_id {
            let snapshot = self.memory_service.load_snapshot(user_id)?;
            self.memory_service
                .build_user_memory_system(&memory_base_prompt, Some(&snapshot))
        } else {
            memory_base_prompt
        };

        Ok(SystemPromptPreview {
            stateless_prompt,
            memory_prompt,
        })
    }

    pub fn preview_agent_prompt_settings(&self) -> AgentPromptSettingsPreview {
        AgentPromptSettingsPreview {
            memory_maintenance_system: self.config.memory.prompts.maintenance_system.clone(),
            memory_maintenance_user_template: self
                .config
                .memory
                .prompts
                .maintenance_user_template
                .clone(),
            skill_learning_system: self.config.skills.prompts.learning_system.clone(),
            skill_learning_user_template: self.config.skills.prompts.learning_user_template.clone(),
        }
    }

    pub async fn preview_mcp_settings(
        &self,
        overrides: crate::domain::chat::models::McpOverrides,
    ) -> Result<McpSettingsPreview> {
        let servers = self
            .mcp_client
            .inspect_servers(Some(&overrides))
            .await?
            .into_iter()
            .map(McpServerPreviewDto::from)
            .collect();
        Ok(McpSettingsPreview { servers })
    }

    fn build_system(&self, user_id: Option<&str>, skill_permissions: &SkillPermissions) -> String {
        let scope = if user_id.is_some() {
            SkillScope::Effective
        } else {
            SkillScope::Shared
        };
        let descriptions = self
            .skill_service
            .list_items(scope, user_id)
            .map(|items| filter_skills_by_permissions(items, skill_permissions))
            .map(|items| self.skill_service.render_descriptions(&items))
            .unwrap_or_else(|_| "(no skills available)".to_string());

        let base = self.harness.render_system_base(&self.repo_root);
        if descriptions == "(no skills available)" {
            base
        } else {
            format!(
                "{base}

Skills available (call load_skill to use):
{descriptions}"
            )
        }
    }

    fn build_llm_request_options(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<serde_json::Value>>,
        stream: bool,
        overrides: &crate::domain::chat::models::LlmOverrides,
    ) -> Result<ChatCompletionRequest> {
        Ok(ChatCompletionRequest {
            model: overrides
                .model_id
                .clone()
                .unwrap_or_else(|| self.config.agent.model_id.clone()),
            messages,
            tools,
            stream,
            temperature: overrides.temperature.or(self.config.agent.temperature),
            max_tokens: Some(overrides.max_tokens.unwrap_or(self.config.agent.max_tokens)),
            top_p: overrides.top_p.or(self.config.agent.top_p),
        })
    }

    async fn load_public_tools_with_overrides(
        &self,
        include_session_tools: bool,
        mcp_overrides: Option<&crate::domain::chat::models::McpOverrides>,
        mcp_tool_selection: &McpToolSelection,
    ) -> Vec<serde_json::Value> {
        let mut tools = static_public_tool_schemas(include_session_tools);
        match self
            .mcp_client
            .list_tool_schemas(mcp_overrides, Some(mcp_tool_selection))
            .await
        {
            Ok(mcp_tools) => tools.extend(mcp_tools),
            Err(error) => {
                tracing::warn!(?error, "failed to load mcp tools; continuing without them")
            }
        }
        if mcp_overrides.is_some_and(|overrides| overrides.has_lazy_endpoints()) {
            tools.extend(lazy_mcp_control_tool_schemas());
        }
        self.harness.apply_tool_descriptions(&mut tools);
        tools
    }

    async fn apply_hitl_gate(
        &self,
        tool_call: &ToolCall,
        arguments: &mut serde_json::Value,
        session: Option<&SessionContext>,
        hitl_overrides: &HitlOverrides,
        active_run_generation: Option<u64>,
        sender: Option<&mpsc::Sender<ChatEvent>>,
    ) -> Option<String> {
        let tool_name = tool_call.function.name.as_str();
        let policy = evaluate_tool_hitl_policy(
            hitl_overrides.enabled,
            &hitl_overrides.default_action,
            &hitl_overrides.rules,
            tool_name,
        );
        match policy {
            HitlPolicyDecision::Allow => None,
            HitlPolicyDecision::Reject { reason } => Some(format!(
                "HITL policy rejected tool call {tool_name}: {reason}"
            )),
            HitlPolicyDecision::RequireApproval { risk_level } => {
                let Some(session) = session else {
                    return Some(format!(
                        "HITL approval required for {tool_name}, but no session is available to route the approval."
                    ));
                };
                let Some(generation) = active_run_generation else {
                    return Some(format!(
                        "HITL approval required for {tool_name}, but no active run generation is available."
                    ));
                };
                let Some(sender) = sender else {
                    return Some(format!(
                        "HITL approval required for {tool_name}. Please rerun in streaming mode to approve or disable HITL for this tool."
                    ));
                };
                let request = match HitlDecisionRequest::new_tool_call(
                    &session.session_id,
                    generation,
                    tool_name,
                    arguments.clone(),
                    risk_level,
                    hitl_overrides.timeout_seconds,
                ) {
                    Ok(request) => request,
                    Err(error) => return Some(format!("HITL request failed: {error}")),
                };
                let approval_id = request.approval_id.clone();
                let rx = session.register_hitl_request(request.clone());
                if let Err(error) = write_hitl_request(&self.repo_root, &request).await {
                    tracing::warn!(?error, approval_id, "failed to persist HITL request");
                }
                if !try_send_stream_event(
                    sender,
                    ChatEvent::ApprovalRequired {
                        approval_id: request.approval_id.clone(),
                        kind: "tool_call".to_string(),
                        title: request.title.clone(),
                        summary: request.summary.clone(),
                        risk_level: request.risk_level.as_str().to_string(),
                        tool_name: request.tool_name.clone(),
                        arguments: request.arguments.clone(),
                    },
                )
                .await
                {
                    return Some(format!(
                        "HITL approval required for {tool_name}, but the client disconnected."
                    ));
                }
                let resolution = session
                    .wait_hitl_resolution(&approval_id, rx, hitl_overrides.timeout_seconds)
                    .await;
                if let Err(error) = write_hitl_resolution(&self.repo_root, &resolution).await {
                    tracing::warn!(?error, approval_id, "failed to persist HITL resolution");
                }
                let _ = try_send_stream_event(
                    sender,
                    ChatEvent::ApprovalResolved {
                        approval_id: approval_id.clone(),
                        status: hitl_status_label(&resolution).to_string(),
                    },
                )
                .await;
                match resolution.status {
                    HitlDecisionStatus::Approved => None,
                    HitlDecisionStatus::Modified => {
                        if let Some(modified) = resolution.modified_arguments {
                            *arguments = modified;
                        }
                        None
                    }
                    HitlDecisionStatus::Rejected => Some(format!(
                        "Tool call {tool_name} was rejected by human operator {}{}.",
                        resolution.resolved_by,
                        resolution
                            .note
                            .as_deref()
                            .filter(|note| !note.trim().is_empty())
                            .map(|note| format!(": {note}"))
                            .unwrap_or_default()
                    )),
                    HitlDecisionStatus::Expired => Some(format!(
                        "Tool call {tool_name} was not executed because HITL approval timed out."
                    )),
                    HitlDecisionStatus::Pending => Some(format!(
                        "Tool call {tool_name} is still pending HITL approval."
                    )),
                }
            }
        }
    }

    async fn prepare_tool_result_for_context(
        &self,
        tool_call: &ToolCall,
        result: String,
    ) -> crate::domain::chat::tool_context::ToolContextResult {
        prepare_tool_result_for_context(
            &self.repo_root,
            &tool_call.function.name,
            &tool_call.id,
            result,
            self.tool_context_budget(),
        )
        .await
    }

    fn tool_context_budget(&self) -> ToolContextBudget {
        ToolContextBudget {
            result_size_chars: self.config.agent.tool_result_size_chars,
            turn_budget_chars: self.config.agent.tool_turn_budget_chars,
            preview_size_chars: self.config.agent.tool_result_preview_chars,
        }
    }

    async fn enforce_tool_turn_budget(
        &self,
        messages: &mut [ChatMessage],
        turn_tool_message_start: usize,
    ) {
        enforce_tool_turn_budget(
            &self.repo_root,
            messages,
            turn_tool_message_start,
            self.tool_context_budget(),
        )
        .await;
    }

    async fn dispatch_public_tool(
        &self,
        tool_call: &ToolCall,
        session: Option<&SessionContext>,
        user_id: Option<&str>,
        memory_mode: bool,
        mcp_overrides: &crate::domain::chat::models::McpOverrides,
        mcp_tool_selection: &mut McpToolSelection,
        skill_usages: &mut Vec<SkillUsage>,
        skill_permissions: &SkillPermissions,
        hitl_overrides: &HitlOverrides,
        active_run_generation: Option<u64>,
        sender: Option<&mpsc::Sender<ChatEvent>>,
    ) -> String {
        let mut arguments =
            serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
                .unwrap_or_else(|_| json!({}));
        let tool_name = tool_call.function.name.as_str();

        if let Some(resolution) = self
            .apply_hitl_gate(
                tool_call,
                &mut arguments,
                session,
                hitl_overrides,
                active_run_generation,
                sender,
            )
            .await
        {
            return resolution;
        }

        if tool_name.starts_with("mcp_") {
            return self
                .mcp_client
                .call_tool(
                    tool_name,
                    arguments,
                    Some(mcp_overrides),
                    Some(mcp_tool_selection),
                )
                .await;
        }

        if tool_name == "read_file" && should_use_mcp_file_reader(&arguments) {
            return self.read_file_via_mcp(&arguments, mcp_overrides).await;
        }

        if tool_name == "compress" {
            return "Compressing...".to_string();
        }

        if let Some(session) = session
            && let Some(result) = self
                .session_service
                .dispatch_session_tool(
                    session,
                    tool_name,
                    &arguments,
                    &self.task_service,
                    &self.llm_client,
                    &self.config.agent.model_id,
                )
                .await
        {
            return result;
        }

        if let Some(result) = self
            .task_service
            .dispatch_public_tool(tool_name, &arguments)
        {
            return result;
        }

        if let Some(result) = self
            .worktree_service
            .dispatch_public_tool_async(tool_name, &arguments)
            .await
        {
            return result;
        }

        if let Some(result) = dispatch_public_file_tool(&self.repo_root, tool_name, &arguments) {
            return result;
        }

        let outcome: Result<String> = match tool_name {
            "task" => {
                let prompt = arguments
                    .get("prompt")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .context("task requires a non-empty prompt");
                let agent_type = arguments
                    .get("agent_type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Explore");
                match prompt {
                    Ok(prompt) => Ok(self.run_subagent(prompt, agent_type).await),
                    Err(error) => Err(error),
                }
            }
            "load_skill" => (|| {
                let name = arguments
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .context("load_skill requires a non-empty name")?;
                let skill = self.skill_service.get_resolved_skill(name, user_id)?;
                if !skill_allowed(&skill.name, skill_permissions) {
                    anyhow::bail!("Skill not allowed for current user: {}", skill.name);
                }
                if memory_mode && user_id.is_some() {
                    skill_usages.push(SkillUsage {
                        name: skill.name.clone(),
                        scope: skill.scope,
                    });
                }
                Ok(format!(
                    "<skill name=\"{}\" scope=\"{}\">\n{}\n</skill>",
                    skill.name,
                    skill.scope.as_str(),
                    skill.body
                ))
            })(),
            SEARCH_LAZY_MCP_TOOLS_TOOL => {
                self.search_lazy_mcp_tools(&arguments, mcp_overrides).await
            }
            ACTIVATE_LAZY_MCP_TOOLS_TOOL => {
                self.activate_lazy_mcp_tools(&arguments, mcp_overrides, mcp_tool_selection)
                    .await
            }
            other => Ok(format!("Unknown tool: {other}")),
        };

        outcome.unwrap_or_else(|error| {
            tracing::warn!(tool = tool_name, ?error, "public tool call failed");
            format!("Error: {error}")
        })
    }

    async fn read_file_via_mcp(
        &self,
        arguments: &serde_json::Value,
        mcp_overrides: &crate::domain::chat::models::McpOverrides,
    ) -> String {
        let path = arguments
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(path) = path else {
            return "Error: read_file requires a non-empty path".to_string();
        };

        let file_path = match safe_workspace_path(&self.repo_root, path) {
            Ok(path) => path,
            Err(error) => return format!("Error: {error}"),
        };
        let file_type = match file_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
        {
            Some(value) => value,
            None => return "Error: Unsupported file type for MCP read".to_string(),
        };

        let bytes = match tokio::fs::read(&file_path).await {
            Ok(bytes) => bytes,
            Err(error) => return format!("Error: failed to read {}: {error}", file_path.display()),
        };

        self.mcp_client
            .call_tool(
                "read_file",
                json!({
                    "file_content": STANDARD.encode(bytes),
                    "file_type": file_type,
                }),
                Some(mcp_overrides),
                None,
            )
            .await
    }

    async fn search_lazy_mcp_tools(
        &self,
        arguments: &serde_json::Value,
        mcp_overrides: &crate::domain::chat::models::McpOverrides,
    ) -> Result<String> {
        let query = arguments
            .get("query")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("search_lazy_mcp_tools requires a non-empty query")?;
        let endpoint_key_filter = arguments
            .get("endpoint_key")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let mut matches = self
            .mcp_client
            .search_lazy_tool_candidates(Some(mcp_overrides), query)
            .await?;
        if let Some(endpoint_key_filter) = endpoint_key_filter {
            matches.retain(|item| item.endpoint_key == endpoint_key_filter);
        }
        if matches.is_empty() {
            return Ok("No matching lazy MCP tools found.".to_string());
        }

        let payload = matches
            .iter()
            .map(|item| {
                json!({
                    "endpoint": item.endpoint,
                    "endpoint_key": item.endpoint_key,
                    "tool_name": item.tool_name,
                    "description": item.description,
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".to_string()))
    }

    async fn activate_lazy_mcp_tools(
        &self,
        arguments: &serde_json::Value,
        mcp_overrides: &crate::domain::chat::models::McpOverrides,
        mcp_tool_selection: &mut McpToolSelection,
    ) -> Result<String> {
        let endpoint_key = arguments
            .get("endpoint_key")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("activate_lazy_mcp_tools requires endpoint_key")?;
        let tool_names = arguments
            .get("tool_names")
            .and_then(|value| value.as_array())
            .context("activate_lazy_mcp_tools requires tool_names array")?
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if tool_names.is_empty() {
            anyhow::bail!("activate_lazy_mcp_tools requires at least one tool name");
        }

        let available = self
            .mcp_client
            .search_lazy_tool_candidates(Some(mcp_overrides), "")
            .await?;
        let matching = available
            .into_iter()
            .filter(|item| item.endpoint_key == endpoint_key)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            anyhow::bail!("No lazy MCP endpoint found for endpoint_key={endpoint_key}");
        }
        let available_names = matching
            .iter()
            .map(|item| item.tool_name.clone())
            .collect::<HashSet<_>>();
        let invalid = tool_names
            .iter()
            .filter(|name| !available_names.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        if !invalid.is_empty() {
            anyhow::bail!(
                "These tools are not available on lazy MCP endpoint {endpoint_key}: {}",
                invalid.join(", ")
            );
        }

        mcp_tool_selection.activate(endpoint_key, &tool_names);
        let endpoint = matching
            .first()
            .map(|item| item.endpoint.clone())
            .unwrap_or_default();
        Ok(format!(
            "Activated {} lazy MCP tool(s) from {} ({endpoint_key}): {}. They will be available in the next round.",
            tool_names.len(),
            endpoint,
            tool_names.join(", ")
        ))
    }

    async fn run_subagent(&self, prompt: &str, agent_type: &str) -> String {
        let mut messages = build_subagent_initial_messages(
            self.harness.render_subagent_system(agent_type),
            prompt,
        );
        let mut final_reply = "(no summary)".to_string();
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        })];
        if agent_type != "Explore" {
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Write file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    }
                }
            }));
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": "edit_file",
                    "description": "Edit file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "old_text": { "type": "string" },
                            "new_text": { "type": "string" }
                        },
                        "required": ["path", "old_text", "new_text"]
                    }
                }
            }));
        }
        self.harness.apply_tool_descriptions(&mut tools);

        for _ in 0..self.config.agent.subagent_max_iterations {
            let response = match self
                .llm_client
                .chat(&ChatCompletionRequest {
                    model: self.config.agent.model_id.clone(),
                    messages: messages.clone(),
                    tools: Some(tools.clone()),
                    stream: false,
                    temperature: self.config.agent.temperature,
                    max_tokens: Some(self.config.agent.max_tokens),
                    top_p: self.config.agent.top_p,
                })
                .await
            {
                Ok(response) => response,
                Err(error) => return format!("Error: {error}"),
            };

            let Some(choice) = response.choices.into_iter().next() else {
                break;
            };
            let assistant = choice.message;
            let tool_calls = assistant.tool_calls.clone();
            final_reply = assistant
                .content
                .clone()
                .unwrap_or_else(|| "(no summary)".to_string());
            messages.push(assistant.into_chat_message());

            if tool_calls.is_empty() {
                break;
            }

            for tool_call in tool_calls {
                let arguments =
                    serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                let output = dispatch_public_file_tool(
                    &self.repo_root,
                    tool_call.function.name.as_str(),
                    &arguments,
                )
                .unwrap_or_else(|| format!("Unknown tool: {}", tool_call.function.name));
                messages.push(ChatMessage::tool(
                    tool_call.id,
                    truncate_for_preview(&output, 50_000),
                ));
            }
        }

        final_reply
    }

    async fn recover_missing_final_reply(
        &self,
        messages: &[ChatMessage],
        overrides: &crate::domain::chat::models::LlmOverrides,
        finish_reason: &str,
    ) -> Result<Option<String>> {
        tracing::warn!(
            finish_reason,
            "attempting final-answer recovery after empty assistant output"
        );

        let mut recovery_messages = messages.to_vec();
        recovery_messages.push(ChatMessage::user(
            self.harness.render_final_answer_recovery(finish_reason),
        ));

        let response = self
            .llm_client
            .chat(&self.build_llm_request_options(recovery_messages, None, false, overrides)?)
            .await?;
        let Some(choice) = response.choices.into_iter().next() else {
            return Ok(None);
        };
        let reply = choice
            .message
            .content
            .unwrap_or_default()
            .trim()
            .to_string();
        if reply.is_empty() {
            return Ok(None);
        }
        Ok(Some(reply))
    }

    async fn finalize_memory_side_effects(
        &self,
        mode: &ChatMode,
        session: Option<&SessionContext>,
        user_id: Option<&str>,
        user_message: &str,
        assistant_reply: &str,
        skill_usages: &[SkillUsage],
        prompt_overrides: &AgentPromptOverrides,
    ) -> Vec<SkillDocument> {
        if !mode.allows_self_evolution() {
            return Vec::new();
        }

        if let Err(error) = self
            .memory_service
            .maintain_after_turn(
                &self.llm_client,
                &self.config.agent.model_id,
                self.config.memory.file_memory.update_max_tokens,
                self.config.agent.max_iterations,
                user_id,
                user_message,
                assistant_reply,
                prompt_overrides
                    .memory_maintenance_system
                    .as_deref()
                    .unwrap_or(self.config.memory.prompts.maintenance_system.as_str()),
                prompt_overrides
                    .memory_maintenance_user_template
                    .as_deref()
                    .unwrap_or(
                        self.config
                            .memory
                            .prompts
                            .maintenance_user_template
                            .as_str(),
                    ),
            )
            .await
        {
            tracing::warn!("memory maintenance failed: {error}");
        }

        let Some(user_id) = user_id else {
            return Vec::new();
        };

        if let Some(session) = session {
            if let Ok(snapshot) = self.memory_service.load_snapshot(user_id) {
                session.put_cached_snapshot(snapshot);
            }
        }
        if !self.config.skills.learning.enabled {
            return Vec::new();
        }

        self.skill_service
            .learn_from_usage(
                &self.llm_client,
                &self.config.agent.model_id,
                self.config.skills.learning.update_max_tokens,
                self.config.agent.max_iterations,
                Some(user_id),
                skill_usages,
                user_message,
                assistant_reply,
                prompt_overrides
                    .skill_learning_system
                    .as_deref()
                    .unwrap_or(self.config.skills.prompts.learning_system.as_str()),
                prompt_overrides
                    .skill_learning_user_template
                    .as_deref()
                    .unwrap_or(self.config.skills.prompts.learning_user_template.as_str()),
            )
            .await
    }

    fn load_memory_snapshot_for_prompt(
        &self,
        session: Option<&SessionContext>,
        user_id: &str,
    ) -> Result<crate::domain::memory::models::UserMemorySnapshot> {
        if let Some(session) = session
            && let Some(snapshot) = session.get_cached_snapshot(user_id)
        {
            return Ok(snapshot);
        }

        let snapshot = self.memory_service.load_snapshot(user_id)?;
        if let Some(session) = session {
            session.put_cached_snapshot(snapshot.clone());
        }
        Ok(snapshot)
    }

    async fn inject_session_messages(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: Option<&SessionContext>,
        active_run_generation: Option<u64>,
    ) -> Result<()> {
        let Some(session) = session else {
            return Ok(());
        };

        let notifications = session.drain_background_notifications();
        if !notifications.is_empty() {
            let text = notifications
                .iter()
                .map(|item| format!("[bg:{}] {}: {}", item.task_id, item.status, item.result))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(ChatMessage::user(format!(
                "<background-results>\n{text}\n</background-results>"
            )));
            messages.push(ChatMessage::assistant(
                Some(self.harness.background_results_ack().to_string()),
                Vec::new(),
            ));
        }

        let inbox = session.read_inbox()?;
        if !inbox.is_empty() {
            messages.push(ChatMessage::user(render_inbox_block("inbox", &inbox)?));
            messages.push(ChatMessage::assistant(
                Some(self.harness.inbox_ack().to_string()),
                Vec::new(),
            ));
        }

        let steering = if let Some(generation) = active_run_generation {
            session.drain_steering_messages(generation)?
        } else {
            Vec::new()
        };
        if !steering.is_empty() {
            messages.push(ChatMessage::user(render_inbox_block(
                "steering", &steering,
            )?));
            messages.push(ChatMessage::assistant(
                Some(self.harness.steering_ack().to_string()),
                Vec::new(),
            ));
        }

        Ok(())
    }

    fn apply_steering_interrupt(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: Option<&SessionContext>,
        active_run_generation: Option<u64>,
        remaining_tool_calls: Option<&[ToolCall]>,
        sender: Option<&mpsc::Sender<ChatEvent>>,
    ) -> Result<bool> {
        let Some(session) = session else {
            return Ok(false);
        };
        let Some(generation) = active_run_generation else {
            return Ok(false);
        };
        let steering = session.drain_steering_messages(generation)?;
        if steering.is_empty() {
            return Ok(false);
        }
        let rendered = render_inbox_block("steering", &steering)?;
        let preview = steering
            .last()
            .and_then(|item| item.get("content"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("收到新的 steering 消息")
            .to_string();
        let skipped_tools = remaining_tool_calls
            .unwrap_or(&[])
            .iter()
            .map(|tool_call| tool_call.function.name.clone())
            .collect::<Vec<_>>();

        tracing::info!(
            session_id = session.session_id,
            message = preview,
            skipped_tools = ?skipped_tools,
            "applying steering interrupt"
        );

        messages.push(ChatMessage::user(rendered));
        messages.push(ChatMessage::assistant(
            Some(self.harness.steering_ack().to_string()),
            Vec::new(),
        ));

        if let Some(sender) = sender {
            let event = ChatEvent::Steering {
                message: preview,
                skipped_tools,
            };
            let sender = sender.clone();
            tokio::spawn(async move {
                let _ = try_send_stream_event(&sender, event).await;
            });
        }

        Ok(true)
    }

    fn apply_todo_reminder(
        &self,
        messages: &mut Vec<ChatMessage>,
        session: Option<&SessionContext>,
        tool_calls: &[ToolCall],
        rounds_without_todo: &mut usize,
    ) {
        let Some(session) = session else {
            return;
        };
        let used_todo = tool_calls
            .iter()
            .any(|tool_call| tool_call.function.name == "TodoWrite");
        *rounds_without_todo = if used_todo {
            0
        } else {
            rounds_without_todo.saturating_add(1)
        };
        if session.todo.has_open_items() && *rounds_without_todo >= 3 {
            messages.push(ChatMessage::user(
                self.harness.todo_reminder_user().to_string(),
            ));
            messages.push(ChatMessage::assistant(
                Some(self.harness.todo_reminder_ack().to_string()),
                Vec::new(),
            ));
        }
    }

    fn spawn_stream_memory_side_effects(
        &self,
        mode: &ChatMode,
        session: Option<Arc<SessionContext>>,
        user_id: Option<String>,
        user_message: String,
        assistant_reply: String,
        skill_usages: Vec<SkillUsage>,
        prompt_overrides: AgentPromptOverrides,
        trace_request: HarnessTraceRequest,
        trace_prompts: HarnessTracePrompts,
        trace_id: String,
        trace_snapshot_id: String,
        started_at_ms: u128,
        run_kind: String,
        finish_reason: String,
        iterations: usize,
        tool_names: Vec<String>,
        final_reply_recovered: bool,
    ) {
        if !mode.allows_self_evolution() {
            return;
        }

        let orchestrator = self.clone();
        tokio::spawn(async move {
            let updated_skills = orchestrator
                .finalize_memory_side_effects(
                    &ChatMode::Memory,
                    session.as_deref(),
                    user_id.as_deref(),
                    &user_message,
                    &assistant_reply,
                    &skill_usages,
                    &prompt_overrides,
                )
                .await;
            let output_files = extract_output_files(&orchestrator.repo_root, &assistant_reply);
            orchestrator
                .persist_harness_trace(build_harness_trace(
                    &trace_request,
                    &trace_prompts,
                    &trace_id,
                    &trace_snapshot_id,
                    started_at_ms,
                    &run_kind,
                    ChatMode::Memory.as_str(),
                    "success".to_string(),
                    finish_reason,
                    None,
                    iterations,
                    &tool_names,
                    assistant_reply.chars().count(),
                    &output_files,
                    &skill_usages,
                    updated_skills.len(),
                    true,
                    final_reply_recovered,
                ))
                .await;
            tracing::info!(
                user_id = user_id.as_deref().unwrap_or("-"),
                updated_skills = updated_skills.len(),
                "background memory side effects finished"
            );
        });
    }

    async fn persist_harness_trace(&self, trace: HarnessRunTrace) {
        if let Err(error) = write_harness_run_trace(&self.repo_root, &trace).await {
            tracing::warn!(
                ?error,
                trace_id = trace.trace_id,
                "failed to persist harness trace"
            );
        }
    }
}

async fn process_llm_sse_frame(
    frame: &str,
    tool_accumulators: &mut Vec<ToolCallAccumulator>,
    round_text: &mut String,
    full_reply: &mut String,
    finish_reason: &mut String,
    sender: &mpsc::Sender<ChatEvent>,
) -> Result<()> {
    for payload in parse_sse_frame(frame)? {
        if payload == "[DONE]" {
            continue;
        }
        let chunk: StreamChunk =
            serde_json::from_str(&payload).context("invalid llm stream json")?;
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                if !content.is_empty() {
                    round_text.push_str(&content);
                    full_reply.push_str(&content);
                    if !try_send_stream_event(sender, ChatEvent::Text(content)).await {
                        return Ok(());
                    }
                }
            }
            for delta in choice.delta.tool_calls {
                while tool_accumulators.len() <= delta.index {
                    tool_accumulators.push(ToolCallAccumulator::default());
                }
                tool_accumulators[delta.index].apply_delta(delta);
            }
            if let Some(reason) = choice.finish_reason {
                *finish_reason = reason;
            }
        }
    }
    Ok(())
}

fn next_sse_frame(buffer: &mut Vec<u8>) -> Result<Option<String>> {
    let Some((delimiter_start, delimiter_len)) = find_sse_frame_delimiter(buffer) else {
        return Ok(None);
    };

    let frame_bytes = buffer[..delimiter_start].to_vec();
    buffer.drain(..delimiter_start + delimiter_len);
    let frame = String::from_utf8(frame_bytes).context("llm stream returned invalid utf-8")?;
    Ok(Some(frame))
}

fn find_sse_frame_delimiter(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
}

async fn try_send_stream_event(sender: &mpsc::Sender<ChatEvent>, event: ChatEvent) -> bool {
    sender.send(event).await.is_ok()
}

fn hitl_status_label(resolution: &HitlDecisionResolution) -> &'static str {
    match resolution.status {
        HitlDecisionStatus::Pending => "pending",
        HitlDecisionStatus::Approved => "approved",
        HitlDecisionStatus::Rejected => "rejected",
        HitlDecisionStatus::Modified => "modified",
        HitlDecisionStatus::Expired => "expired",
    }
}

fn static_public_tool_schemas(include_session_tools: bool) -> Vec<serde_json::Value> {
    let mut tools = public_file_tool_schemas();
    tools.extend(public_compaction_tool_schemas());
    tools.extend(public_task_tool_schemas());
    tools.extend(public_worktree_tool_schemas());
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "task",
            "description": "Spawn a subagent for isolated exploration or work. Returns a summary.",
            "parameters": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "agent_type": { "type": "string", "enum": ["Explore", "general-purpose"] }
                },
                "required": ["prompt"]
            }
        }
    }));
    if include_session_tools {
        tools.extend(SessionService::session_tool_schemas());
    }
    tools.push(json!({
        "type": "function",
        "function": {
            "name": "load_skill",
            "description": "Load the full content of a named skill when the task requires it.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The skill name to load." }
                },
                "required": ["name"]
            }
        }
    }));
    tools
}

fn lazy_mcp_control_tool_schemas() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": SEARCH_LAZY_MCP_TOOLS_TOOL,
                "description": "Search hidden lazy MCP tools by keyword before exposing them to the model.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What capability or tool you need." },
                        "endpoint_key": { "type": "string", "description": "Optional lazy MCP endpoint key to narrow the search." }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": ACTIVATE_LAZY_MCP_TOOLS_TOOL,
                "description": "Expose a small subset of tools from one lazy MCP endpoint for the next reasoning round.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "endpoint_key": { "type": "string", "description": "The lazy MCP endpoint key returned by search_lazy_mcp_tools." },
                        "tool_names": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The specific tool names to expose."
                        }
                    },
                    "required": ["endpoint_key", "tool_names"]
                }
            }
        }),
    ]
}

#[derive(Clone)]
struct PreparedRequest {
    messages: Vec<ChatMessage>,
    cleaned_history: Vec<HistoryEntry>,
    user_message: String,
    session: Option<Arc<SessionContext>>,
    user_id: Option<String>,
    llm_overrides: crate::domain::chat::models::LlmOverrides,
    prompt_overrides: AgentPromptOverrides,
    mcp_overrides: crate::domain::chat::models::McpOverrides,
    skill_permissions: SkillPermissions,
    hitl_overrides: HitlOverrides,
    trace_request: HarnessTraceRequest,
    trace_prompts: HarnessTracePrompts,
    trace_snapshot_id: String,
}

fn skill_allowed(name: &str, permissions: &SkillPermissions) -> bool {
    let normalized = name.trim();
    if normalized.is_empty() {
        return false;
    }
    if permissions
        .denied_skills
        .iter()
        .any(|item| item.trim() == normalized)
    {
        return false;
    }
    permissions.allowed_skills.is_empty()
        || permissions
            .allowed_skills
            .iter()
            .any(|item| item.trim() == normalized)
}

fn filter_skills_by_permissions(
    items: Vec<SkillDocument>,
    permissions: &SkillPermissions,
) -> Vec<SkillDocument> {
    items
        .into_iter()
        .filter(|item| skill_allowed(&item.name, permissions))
        .collect()
}

fn append_uploaded_files(
    message: String,
    files: &[crate::domain::chat::models::UploadedFile],
) -> String {
    if files.is_empty() {
        return message;
    }

    let mut content = message;
    content.push_str("\n\n**上传的文件：**\n");
    for file in files {
        content.push_str(&format!(
            "- {} (路径: {}, 大小: {} bytes{})\n",
            file.original_name,
            file.saved_path,
            file.size,
            file.content_type
                .as_deref()
                .map(|value| format!(", 类型: {value}"))
                .unwrap_or_default()
        ));
    }
    content
}

fn build_response_history(
    history: &[HistoryEntry],
    user_message: &str,
    assistant_reply: &str,
) -> Vec<HistoryEntry> {
    let mut output = history.to_vec();
    output.push(HistoryEntry {
        role: "user".to_string(),
        content: user_message.to_string(),
    });
    output.push(HistoryEntry {
        role: "assistant".to_string(),
        content: assistant_reply.to_string(),
    });
    output
}

fn append_reply_segment(reply: &mut String, segment: &str) {
    if segment.trim().is_empty() {
        return;
    }
    if reply.trim().is_empty() {
        *reply = segment.to_string();
        return;
    }
    reply.push_str("\n\n");
    reply.push_str(segment);
}

fn render_inbox_block(tag: &str, items: &[serde_json::Value]) -> Result<String> {
    Ok(format!(
        "<{tag}>{}</{tag}>",
        serde_json::to_string_pretty(items).context("failed to encode inbox messages")?
    ))
}

fn build_subagent_initial_messages(system_prompt: String, prompt: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user(prompt.to_string()),
    ]
}

fn append_system_instruction(base_system: String, system_append: Option<&str>) -> String {
    let Some(system_append) = system_append
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return base_system;
    };

    format!(
        "{base_system}\n\n<custom_agent_instruction>\nUser-configured supplemental instruction. Follow it unless it conflicts with higher-priority system constraints or tool safety rules.\n{system_append}\n</custom_agent_instruction>"
    )
}

fn resolve_max_iterations(
    overrides: &crate::domain::chat::models::LlmOverrides,
    default_max_iterations: usize,
) -> usize {
    overrides.max_iterations.unwrap_or(default_max_iterations)
}

fn max_iterations_from_request(
    overrides: &crate::domain::chat::models::LlmOverrides,
    default_max_iterations: usize,
) -> usize {
    resolve_max_iterations(overrides, default_max_iterations)
}

fn build_trace_request(
    request: &ChatRequest,
    mode: &ChatMode,
    config: &AppConfig,
    resolved_max_iterations: usize,
) -> HarnessTraceRequest {
    HarnessTraceRequest {
        session_id: request.session_id.clone(),
        user_id_present: request.user_id.is_some(),
        history_items: request.history.len(),
        uploaded_files: request.files.len(),
        memory_snapshot_injected: matches!(mode, ChatMode::Memory) && request.user_id.is_some(),
        self_evolution_allowed: mode.allows_self_evolution(),
        resolved_model_id: request
            .llm_overrides
            .model_id
            .clone()
            .unwrap_or_else(|| config.agent.model_id.clone()),
        resolved_max_iterations,
        temperature: request
            .llm_overrides
            .temperature
            .or(config.agent.temperature),
        top_p: request.llm_overrides.top_p.or(config.agent.top_p),
        mcp_base_urls: request.mcp_overrides.base_urls.len(),
        mcp_disabled_urls: request.mcp_overrides.disabled_urls.len(),
        mcp_lazy_urls: request.mcp_overrides.lazy_urls.len(),
    }
}

fn build_trace_prompts(
    repo_root: &Path,
    harness: &HarnessAssets,
    request: &ChatRequest,
    _mode: &ChatMode,
) -> HarnessTracePrompts {
    HarnessTracePrompts {
        top_level_system: if request.system_override.is_some() || request.system.is_some() {
            PromptSource::request()
        } else {
            harness.system_base_source().clone()
        },
        system_append: if request.system_append.is_some() {
            PromptSource::request()
        } else {
            PromptSource::none()
        },
        final_answer_recovery: harness.final_answer_recovery_source().clone(),
        subagent_shared: harness.subagent_shared_source().clone(),
        subagent_explore: harness.subagent_explore_source().clone(),
        subagent_general: harness.subagent_general_source().clone(),
        memory_maintenance_system: resolve_prompt_source(
            request.prompt_overrides.memory_maintenance_system.as_ref(),
            repo_root,
            MEMORY_MAINTENANCE_SYSTEM_PATH,
        ),
        memory_maintenance_user_template: resolve_prompt_source(
            request
                .prompt_overrides
                .memory_maintenance_user_template
                .as_ref(),
            repo_root,
            MEMORY_MAINTENANCE_USER_TEMPLATE_PATH,
        ),
        skill_learning_system: resolve_prompt_source(
            request.prompt_overrides.skill_learning_system.as_ref(),
            repo_root,
            SKILL_LEARNING_SYSTEM_PATH,
        ),
        skill_learning_user_template: resolve_prompt_source(
            request
                .prompt_overrides
                .skill_learning_user_template
                .as_ref(),
            repo_root,
            SKILL_LEARNING_USER_TEMPLATE_PATH,
        ),
    }
}

fn resolve_prompt_source(
    request_override: Option<&String>,
    repo_root: &Path,
    relative_path: &str,
) -> PromptSource {
    if request_override.is_some() {
        PromptSource::request()
    } else {
        resolve_repo_prompt_source(repo_root, relative_path)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_harness_trace(
    request: &HarnessTraceRequest,
    prompts: &HarnessTracePrompts,
    trace_id: &str,
    harness_snapshot_id: &str,
    started_at_ms: u128,
    run_kind: &str,
    mode: &str,
    status: String,
    finish_reason: String,
    error: Option<String>,
    iterations: usize,
    tool_names: &[String],
    reply_chars: usize,
    output_files: &[OutputFile],
    skill_usages: &[SkillUsage],
    skills_updated: usize,
    self_evolution_executed: bool,
    final_reply_recovered: bool,
) -> HarnessRunTrace {
    HarnessRunTrace {
        trace_id: trace_id.to_string(),
        harness_snapshot_id: harness_snapshot_id.to_string(),
        started_at_ms,
        finished_at_ms: now_ms(),
        run_kind: run_kind.to_string(),
        mode: mode.to_string(),
        request: request.clone(),
        prompts: prompts.clone(),
        outcome: HarnessTraceOutcome {
            status,
            finish_reason,
            error,
            iterations,
            tool_calls: tool_names.len(),
            tool_names: tool_names.to_vec(),
            reply_chars,
            output_files: output_files.len(),
            output_file_names: output_files.iter().map(|item| item.name.clone()).collect(),
            used_skill_names: unique_skill_names(skill_usages),
            skills_updated,
            final_reply_recovered,
            self_evolution_executed,
        },
    }
}

fn unique_skill_names(usages: &[SkillUsage]) -> Vec<String> {
    let mut names = usages
        .iter()
        .map(|item| item.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn build_empty_stream_reply_error(finish_reason: &str, max_iterations: usize) -> String {
    if finish_reason == "max_iterations" {
        return format!(
            "模型在 {max_iterations} 轮工具/推理后仍未生成可展示内容。请重试，或调高当前最大轮次设置（前端 Max Iterations / 后端 AGENT_MAX_ITERATIONS）。"
        );
    }

    format!(
        "模型返回了空白流式结果（finish_reason: {finish_reason}）。系统已尝试补救生成最终答案，但仍未得到可展示内容，请重试。"
    )
}

fn log_agent_input(mode: &ChatMode, run_kind: &str, request: &ChatRequest) {
    let request_json = serde_json::to_string_pretty(request)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{error}\"}}"));
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = request.session_id.as_deref().unwrap_or("-"),
        user_id = request.user_id.as_deref().unwrap_or("-"),
        history_items = request.history.len(),
        uploaded_files = request.files.len(),
        "agent input follows\n{}",
        request_json
    );
}

fn log_agent_iteration_start(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    messages_before_iteration: usize,
) {
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        messages_before_iteration,
        "agent iteration started"
    );
}

fn log_llm_request(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    request: &ChatCompletionRequest,
) {
    let request_json = serde_json::to_string_pretty(request)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{error}\"}}"));
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        model = request.model,
        stream = request.stream,
        message_count = request.messages.len(),
        tool_count = request.tools.as_ref().map(Vec::len).unwrap_or(0),
        "llm request follows\n{}",
        request_json
    );
}

fn log_llm_response(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    finish_reason: &str,
    assistant_content: &str,
    tool_calls: &[ToolCall],
) {
    let response_json = serde_json::to_string_pretty(&json!({
        "finish_reason": finish_reason,
        "message": {
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": tool_calls,
        }
    }))
    .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{error}\"}}"));
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        finish_reason,
        assistant_content_chars = assistant_content.chars().count(),
        tool_call_count = tool_calls.len(),
        "llm response follows\n{}",
        response_json
    );
}

fn log_tool_call(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    tool_index: usize,
    tool_call: &ToolCall,
) {
    let tool_call_json = serde_json::to_string_pretty(tool_call)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{error}\"}}"));
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        tool_index,
        tool = tool_call.function.name,
        arguments_chars = tool_call.function.arguments.chars().count(),
        "agent tool call follows\n{}",
        tool_call_json
    );
}

fn log_tool_result(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    tool_index: usize,
    tool_name: &str,
    result: &str,
) {
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        tool_index,
        tool = tool_name,
        result_chars = result.chars().count(),
        "agent tool result follows\n{}",
        result
    );
}

fn log_tool_context_result(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    run_kind: &str,
    iteration: usize,
    tool_index: usize,
    tool_name: &str,
    context_result: &crate::domain::chat::tool_context::ToolContextResult,
) {
    tracing::info!(
        mode = mode.as_str(),
        run_kind,
        session_id = session_id_or_dash(session),
        user_id = user_id.unwrap_or("-"),
        iteration,
        tool_index,
        tool = tool_name,
        model_content_chars = context_result.model_content.chars().count(),
        persisted_path = context_result
            .persisted_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string()),
        "agent tool context result follows\n{}",
        context_result.model_content
    );
}

fn log_final_reply(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    finish_reason: &str,
    assistant_reply: &str,
) {
    let mode = mode.as_str();
    let session_id = session_id_or_dash(session);
    let user_id = user_id.unwrap_or("-");
    let reply_chars = assistant_reply.chars().count();

    if assistant_reply.is_empty() {
        tracing::info!(
            mode,
            session_id,
            user_id,
            finish_reason,
            reply_chars,
            "llm final reply is empty"
        );
        return;
    }

    tracing::info!(
        mode,
        session_id,
        user_id,
        finish_reason,
        reply_chars,
        "llm final reply follows\n{}",
        assistant_reply
    );
}

fn session_id_or_dash(session: Option<&SessionContext>) -> &str {
    session
        .map(|session| session.session_id.as_str())
        .unwrap_or("-")
}

fn should_use_mcp_file_reader(arguments: &serde_json::Value) -> bool {
    let Some(path) = arguments.get("path").and_then(|value| value.as_str()) else {
        return false;
    };
    matches!(
        Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("docx" | "xlsx" | "xls" | "csv")
    )
}

fn strip_cross_session_injections(history: Vec<HistoryEntry>) -> Vec<HistoryEntry> {
    let mut cleaned = Vec::new();
    let mut skip_ack = false;
    for entry in history {
        if entry.role == "user"
            && entry.content.contains(&format!("<{PAST_CONTEXT_TAG}>"))
            && entry.content.contains(&format!("</{PAST_CONTEXT_TAG}>"))
        {
            skip_ack = true;
            continue;
        }

        if skip_ack {
            if entry.role == "assistant" && entry.content == PAST_CONTEXT_ACK {
                skip_ack = false;
                continue;
            }
            skip_ack = false;
        }

        cleaned.push(entry);
    }
    cleaned
}

fn parse_sse_frame(frame: &str) -> Result<Vec<String>> {
    let mut payloads = Vec::new();
    for line in frame.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            payloads.push(data.trim().to_string());
        }
    }
    Ok(payloads)
}

fn truncate_for_preview(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    format!(
        "{}...[truncated]",
        text.chars().take(limit).collect::<String>().trim_end()
    )
}

fn extract_output_files(repo_root: &Path, reply: &str) -> Vec<OutputFile> {
    let pattern = Regex::new(r#"([^\s'"，。]+\.(?:docx|doc|md|xlsx|xls|csv))"#)
        .expect("valid output file regex");
    let outputs_dir = repo_root.join("outputs");
    let outputs_root = outputs_dir.canonicalize().unwrap_or(outputs_dir);
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for captures in pattern.captures_iter(reply) {
        let Some(raw_path) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        let candidate = PathBuf::from(raw_path);
        let resolved = if candidate.is_absolute() {
            candidate
        } else {
            repo_root.join(raw_path)
        };
        if !resolved.exists() || !resolved.is_file() {
            continue;
        }
        let Ok(canonical_path) = resolved.canonicalize() else {
            continue;
        };
        if !canonical_path.starts_with(&outputs_root) {
            continue;
        }
        let canonical = canonical_path.display().to_string();
        if !seen.insert(canonical.clone()) {
            continue;
        }
        files.push(OutputFile {
            name: canonical_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(raw_path)
                .to_string(),
            path: canonical,
        });
    }

    files
}

#[cfg(test)]
mod tests {
    use super::{
        ChatMode, append_system_instruction, build_subagent_initial_messages, extract_output_files,
        log_agent_input, log_final_reply, log_llm_request, log_llm_response, log_tool_call,
        log_tool_context_result, log_tool_result, resolve_max_iterations,
        static_public_tool_schemas,
    };
    use crate::config::model::AppConfig;
    use crate::domain::chat::models::{
        AgentPromptOverrides, ChatRequest, HitlOverrides, LlmOverrides, McpOverrides,
        SkillPermissions,
    };
    use crate::domain::chat::orchestrator::ChatOrchestrator;
    use crate::domain::chat::tool_context::ToolContextResult;
    use crate::domain::events::service::EventService;
    use crate::domain::harness::HarnessAssets;
    use crate::domain::memory::service::UserMemoryService;
    use crate::domain::session::service::SessionService;
    use crate::domain::skills::service::SkillService;
    use crate::domain::tasks::service::TaskService;
    use crate::domain::worktree::service::WorktreeService;
    use crate::infra::fs::skill_store::FileSkillStore;
    use crate::infra::fs::user_memory_store::FileMemoryStore;
    use crate::infra::llm::client::LlmClient;
    use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage, FunctionCall, ToolCall};
    use crate::infra::mcp::client::McpClient;
    use std::collections::HashSet;
    use std::fs;
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn static_public_tool_surface_matches_reference_set_except_dynamic_mcp() {
        let actual = static_public_tool_schemas(true)
            .into_iter()
            .filter_map(|tool| {
                tool.get("function")
                    .and_then(|value| value.get("name"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .collect::<HashSet<_>>();

        let expected = [
            "TodoWrite",
            "background_run",
            "broadcast",
            "check_background",
            "claim_task",
            "compress",
            "edit_file",
            "list_teammates",
            "load_skill",
            "plan_approval",
            "read_file",
            "read_inbox",
            "send_message",
            "shutdown_request",
            "spawn_teammate",
            "task",
            "task_bind_worktree",
            "task_create",
            "task_get",
            "task_list",
            "task_update",
            "worktree_create",
            "worktree_events",
            "worktree_keep",
            "worktree_list",
            "worktree_remove",
            "worktree_run",
            "worktree_status",
            "write_file",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn logs_full_final_reply_content() {
        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .without_time()
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            log_final_reply(
                &ChatMode::Memory,
                None,
                Some("user-123"),
                "stop",
                "第一行回复\n第二行回复",
            );
        });

        let output = writer.contents();
        assert!(output.contains("llm final reply follows"));
        assert!(output.contains("第一行回复"));
        assert!(output.contains("第二行回复"));
        assert!(output.contains("memory"));
        assert!(output.contains("user_id"));
        assert!(output.contains("user-123"));
        assert!(output.contains("finish_reason"));
        assert!(output.contains("stop"));
    }

    #[test]
    fn logs_agent_input_and_intermediate_steps() {
        let writer = SharedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .without_time()
            .with_ansi(false)
            .finish();

        let tool_call = ToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"README.md"}"#.to_string(),
            },
        };
        let request = ChatRequest {
            message: "请读取 README".to_string(),
            history: vec![super::HistoryEntry {
                role: "user".to_string(),
                content: "之前的问题".to_string(),
            }],
            system: Some("system prompt".to_string()),
            system_override: None,
            system_append: Some("append prompt".to_string()),
            session_id: Some("session-1".to_string()),
            user_id: Some("user-1".to_string()),
            files: Vec::new(),
            llm_overrides: LlmOverrides {
                model_id: Some("test-model".to_string()),
                temperature: Some(0.1),
                max_tokens: Some(128),
                max_iterations: Some(3),
                top_p: Some(0.9),
            },
            prompt_overrides: AgentPromptOverrides::default(),
            mcp_overrides: McpOverrides::default(),
            skill_permissions: SkillPermissions::default(),
            hitl_overrides: HitlOverrides::default(),
        };
        let llm_request = ChatCompletionRequest {
            model: "test-model".to_string(),
            messages: vec![
                ChatMessage::system("system prompt"),
                ChatMessage::user("hello"),
            ],
            tools: Some(vec![serde_json::json!({
                "type": "function",
                "function": { "name": "read_file" }
            })]),
            stream: false,
            temperature: Some(0.1),
            max_tokens: Some(128),
            top_p: Some(0.9),
        };
        let context_result = ToolContextResult {
            model_content: "tool output for model".to_string(),
            persisted_path: None,
        };

        tracing::subscriber::with_default(subscriber, || {
            log_agent_input(&ChatMode::Stateless, "sync", &request);
            log_llm_request(
                &ChatMode::Stateless,
                None,
                Some("user-1"),
                "sync",
                1,
                &llm_request,
            );
            log_llm_response(
                &ChatMode::Stateless,
                None,
                Some("user-1"),
                "sync",
                1,
                "tool_calls",
                "",
                std::slice::from_ref(&tool_call),
            );
            log_tool_call(
                &ChatMode::Stateless,
                None,
                Some("user-1"),
                "sync",
                1,
                1,
                &tool_call,
            );
            log_tool_result(
                &ChatMode::Stateless,
                None,
                Some("user-1"),
                "sync",
                1,
                1,
                "read_file",
                "README content",
            );
            log_tool_context_result(
                &ChatMode::Stateless,
                None,
                Some("user-1"),
                "sync",
                1,
                1,
                "read_file",
                &context_result,
            );
        });

        let output = writer.contents();
        assert!(output.contains("agent input follows"));
        assert!(output.contains("请读取 README"));
        assert!(output.contains("llm request follows"));
        assert!(output.contains("system prompt"));
        assert!(output.contains("llm response follows"));
        assert!(output.contains("agent tool call follows"));
        assert!(output.contains("README.md"));
        assert!(output.contains("agent tool result follows"));
        assert!(output.contains("README content"));
        assert!(output.contains("agent tool context result follows"));
        assert!(output.contains("tool output for model"));
    }

    #[test]
    fn subagent_starts_with_isolated_context_only() {
        let repo = TestRepo::new();
        let harness =
            HarnessAssets::load(&repo.root, &crate::config::model::AppConfig::default()).unwrap();
        let messages = build_subagent_initial_messages(
            harness.render_subagent_system("Explore"),
            "inspect README",
        );
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("isolated subagent")
        );
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("Stay read-only")
        );
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content.as_deref(), Some("inspect README"));
    }

    #[test]
    fn recovery_prompt_forces_final_answer_without_tools() {
        let repo = TestRepo::new();
        let harness =
            HarnessAssets::load(&repo.root, &crate::config::model::AppConfig::default()).unwrap();
        let prompt = harness.render_final_answer_recovery("max_iterations");
        assert!(prompt.contains("without any user-visible answer"));
        assert!(prompt.contains("finish_reason: max_iterations"));
        assert!(prompt.contains("Do not call tools"));
    }

    #[test]
    fn preview_system_prompt_uses_configured_agent_template() {
        let repo = TestRepo::new();
        let mut orchestrator = build_test_orchestrator(&repo.root);
        orchestrator.config.agent.system_prompt =
            "custom-system {repo_root} -> {outputs_dir}".to_string();
        orchestrator.harness = HarnessAssets::load(&repo.root, &orchestrator.config).unwrap();

        let preview = orchestrator.preview_system_prompts(None).unwrap();

        assert!(preview.stateless_prompt.contains("custom-system"));
        assert!(
            preview
                .stateless_prompt
                .contains(&repo.root.display().to_string())
        );
        assert!(
            preview
                .stateless_prompt
                .contains(&repo.root.join("outputs").display().to_string())
        );
    }

    #[test]
    fn request_override_max_iterations_takes_precedence() {
        let overrides = LlmOverrides {
            model_id: None,
            temperature: None,
            max_tokens: None,
            max_iterations: Some(6),
            top_p: None,
        };
        assert_eq!(resolve_max_iterations(&overrides, 12), 6);
        assert_eq!(
            resolve_max_iterations(
                &LlmOverrides {
                    max_iterations: None,
                    ..overrides
                },
                12
            ),
            12
        );
    }

    #[test]
    fn appends_custom_system_instruction_in_wrapped_block() {
        let prompt = append_system_instruction(
            "base prompt".to_string(),
            Some("Always summarize risks first."),
        );
        assert!(prompt.contains("base prompt"));
        assert!(prompt.contains("<custom_agent_instruction>"));
        assert!(prompt.contains("Always summarize risks first."));
    }

    #[test]
    fn system_override_takes_precedence_over_default_base_prompt() {
        let prompt = append_system_instruction("override prompt".to_string(), Some("appendix"));
        assert!(prompt.starts_with("override prompt"));
        assert!(prompt.contains("appendix"));
    }

    #[test]
    fn steering_interrupt_injects_follow_up_and_reports_skipped_tools() {
        let repo = TestRepo::new();
        let orchestrator = build_test_orchestrator(&repo.root);
        let session_service = SessionService::new(repo.root.clone());
        let session = session_service.get_or_create("steering-test").unwrap();
        let generation = session.begin_agent_run();
        session
            .push_steering_message("先暂停剩余工具，重新评估", generation)
            .unwrap();

        let mut messages = vec![ChatMessage::system("system")];
        let tool_calls = vec![
            ToolCall {
                id: "tool-1".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            },
            ToolCall {
                id: "tool-2".to_string(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: "write_file".to_string(),
                    arguments: "{}".to_string(),
                },
            },
        ];

        let interrupted = orchestrator
            .apply_steering_interrupt(
                &mut messages,
                Some(session.as_ref()),
                Some(generation),
                Some(&tool_calls[1..]),
                None,
            )
            .unwrap();

        assert!(interrupted);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, "user");
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("<steering>")
        );
        assert!(
            messages[1]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("先暂停剩余工具，重新评估")
        );
        assert_eq!(messages[2].role, "assistant");
        assert_eq!(
            messages[2].content.as_deref(),
            Some(orchestrator.harness.steering_ack())
        );
        assert!(
            session
                .drain_steering_messages(generation)
                .unwrap()
                .is_empty()
        );
        session.end_agent_run(generation);
    }

    #[test]
    fn steering_interrupt_uses_custom_harness_ack_message() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("harness/middleware")).unwrap();
        fs::write(
            repo.root.join("harness/middleware/messages.json"),
            r#"{ "steering_ack": "Custom steering ack" }"#,
        )
        .unwrap();

        let orchestrator = build_test_orchestrator(&repo.root);
        let session_service = SessionService::new(repo.root.clone());
        let session = session_service.get_or_create("steering-custom").unwrap();
        let generation = session.begin_agent_run();
        session
            .push_steering_message("暂停剩余工具", generation)
            .unwrap();

        let mut messages = vec![ChatMessage::system("system")];
        let interrupted = orchestrator
            .apply_steering_interrupt(
                &mut messages,
                Some(session.as_ref()),
                Some(generation),
                None,
                None,
            )
            .unwrap();

        assert!(interrupted);
        assert_eq!(messages[2].content.as_deref(), Some("Custom steering ack"));
        session.end_agent_run(generation);
    }

    #[test]
    fn append_reply_segment_concatenates_multiple_visible_replies() {
        let mut reply = String::new();
        super::append_reply_segment(&mut reply, "第一段");
        super::append_reply_segment(&mut reply, "");
        super::append_reply_segment(&mut reply, "第二段");
        assert_eq!(reply, "第一段\n\n第二段");
    }

    #[test]
    fn sse_frame_decoder_waits_for_complete_utf8_frame() {
        let mut buffer = b"data: {\"choices\":[{\"delta\":{\"content\":\"".to_vec();
        let chinese = "中".as_bytes();
        buffer.extend_from_slice(&chinese[..2]);

        assert!(super::next_sse_frame(&mut buffer).unwrap().is_none());

        buffer.extend_from_slice(&chinese[2..]);
        buffer.extend_from_slice(b"\"},\"finish_reason\":null}]}\n\n");

        let frame = super::next_sse_frame(&mut buffer).unwrap().unwrap();

        assert!(frame.contains("中"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn sse_frame_decoder_accepts_crlf_delimiters() {
        let mut buffer = b"data: [DONE]\r\n\r\nnext".to_vec();

        let frame = super::next_sse_frame(&mut buffer).unwrap().unwrap();

        assert_eq!(frame, "data: [DONE]");
        assert_eq!(buffer, b"next");
    }

    #[test]
    fn extract_output_files_only_returns_files_from_outputs_directory() {
        let repo = TestRepo::new();
        let outputs = repo.root.join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        let report = outputs.join("report.md");
        fs::write(&report, "report").unwrap();
        fs::write(repo.root.join("USER.md"), "profile").unwrap();
        fs::write(repo.root.join("MEMORY.md"), "memory").unwrap();

        let reply = format!(
            "已生成 {}，并更新了 {} 和 {}。",
            report.display(),
            repo.root.join("USER.md").display(),
            repo.root.join("MEMORY.md").display()
        );

        let files = extract_output_files(&repo.root, &reply);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "report.md");
        assert_eq!(
            PathBuf::from(&files[0].path),
            report.canonicalize().unwrap()
        );
    }

    #[test]
    fn extract_output_files_ignores_memory_files_named_in_plain_text() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("outputs")).unwrap();
        fs::write(repo.root.join("USER.md"), "profile").unwrap();
        fs::write(repo.root.join("MEMORY.md"), "memory").unwrap();

        let files = extract_output_files(&repo.root, "updated USER.md and MEMORY.md");
        assert!(files.is_empty());
    }

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-output-files-{}-{}",
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

    fn build_test_orchestrator(repo_root: &std::path::Path) -> ChatOrchestrator {
        let config = AppConfig::default();
        let repo_root = repo_root.to_path_buf();
        let memory_store =
            FileMemoryStore::new(repo_root.clone(), config.memory.file_memory.clone());
        let skill_store = FileSkillStore::new(repo_root.join("skills"), memory_store.clone());
        let memory_service = UserMemoryService::new(memory_store);
        let skill_service = SkillService::new(skill_store);
        let event_service = EventService::new(repo_root.clone()).unwrap();
        let task_service = TaskService::new(repo_root.clone()).unwrap();
        let worktree_service =
            WorktreeService::new(repo_root.clone(), task_service.clone(), event_service).unwrap();
        let session_service = SessionService::new(repo_root.clone());
        let harness = HarnessAssets::load(&repo_root, &config).unwrap();
        let llm_client = LlmClient::new(&config).unwrap();
        let mcp_client = McpClient::new(&config).unwrap();
        ChatOrchestrator::new(
            repo_root,
            config,
            harness,
            llm_client,
            mcp_client,
            memory_service,
            skill_service,
            task_service,
            worktree_service,
            session_service,
        )
    }

    #[derive(Clone, Default)]
    struct SharedWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedWriter {
        fn contents(&self) -> String {
            String::from_utf8(
                self.buffer
                    .lock()
                    .expect("log buffer lock poisoned")
                    .clone(),
            )
            .expect("log buffer must be utf-8")
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard {
                buffer: self.buffer.clone(),
            }
        }
    }

    struct SharedWriterGuard {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer
                .lock()
                .expect("log buffer lock poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
