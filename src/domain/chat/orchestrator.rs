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
    HistoryEntry, OutputFile, SkillUsage, SystemPromptPreview,
};
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

const PAST_CONTEXT_TAG: &str = "past_context";
const PAST_CONTEXT_ACK: &str = "Noted. I'll reference this context only if relevant.";

#[derive(Clone)]
pub struct ChatOrchestrator {
    repo_root: PathBuf,
    config: AppConfig,
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
        let prepared = self.prepare_request(request, &mode)?;
        let max_iterations =
            resolve_max_iterations(&prepared.llm_overrides, self.config.agent.max_iterations);
        let mut messages = prepared.messages;
        let tools = self.load_public_tools(prepared.session.is_some()).await;
        let mut reply = String::new();
        let mut finish_reason = "stop".to_string();
        let mut rounds_without_todo = 0usize;

        for _ in 0..max_iterations {
            microcompact(&mut messages);
            if estimate_tokens(&messages) > self.config.agent.auto_compact_token_threshold {
                messages =
                    auto_compact(&self.repo_root, &self.config, &self.llm_client, messages).await?;
            }
            self.inject_session_messages(&mut messages, prepared.session.as_deref())
                .await?;
            let response = self
                .llm_client
                .chat(&self.build_llm_request_options(
                    messages.clone(),
                    Some(tools.clone()),
                    false,
                    &prepared.llm_overrides,
                )?)
                .await?;
            let Some(choice) = response.choices.into_iter().next() else {
                finish_reason = "empty".to_string();
                break;
            };
            finish_reason = choice.finish_reason.unwrap_or_else(|| "stop".to_string());
            let assistant = choice.message;
            let tool_calls = assistant.tool_calls.clone();
            let assistant_content = assistant.content.clone().unwrap_or_default();
            messages.push(assistant.into_chat_message());
            if tool_calls.is_empty() {
                reply = assistant_content;
                break;
            }

            let mut compress_requested = false;
            for tool_call in &tool_calls {
                let result = self
                    .dispatch_public_tool(
                        tool_call,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        matches!(mode, ChatMode::Memory),
                        &mut skill_usages,
                    )
                    .await;
                if tool_call.function.name == "compress" {
                    compress_requested = true;
                }
                messages.push(ChatMessage::tool(tool_call.id.clone(), result));
            }

            self.apply_todo_reminder(
                &mut messages,
                prepared.session.as_deref(),
                &tool_calls,
                &mut rounds_without_todo,
            );

            if compress_requested {
                messages =
                    auto_compact(&self.repo_root, &self.config, &self.llm_client, messages).await?;
            }
        }

        if reply.trim().is_empty() {
            if let Some(recovered_reply) = self
                .recover_missing_final_reply(&messages, &prepared.llm_overrides, &finish_reason)
                .await?
            {
                reply = recovered_reply;
                finish_reason = "stop".to_string();
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
        let prepared = self.prepare_request(request, &mode)?;
        let max_iterations =
            resolve_max_iterations(&prepared.llm_overrides, self.config.agent.max_iterations);
        let mut messages = prepared.messages;
        let tools = self.load_public_tools(prepared.session.is_some()).await;
        let mut full_reply = String::new();
        let mut rounds_without_todo = 0usize;

        for _ in 0..max_iterations {
            if sender.is_closed() {
                tracing::info!("stream receiver closed before next iteration");
                return Ok(());
            }
            microcompact(&mut messages);
            if estimate_tokens(&messages) > self.config.agent.auto_compact_token_threshold {
                messages =
                    auto_compact(&self.repo_root, &self.config, &self.llm_client, messages).await?;
            }
            self.inject_session_messages(&mut messages, prepared.session.as_deref())
                .await?;
            let request_body = self.build_llm_request_options(
                messages.clone(),
                Some(tools.clone()),
                true,
                &prepared.llm_overrides,
            )?;
            let mut stream = self.llm_client.stream_chat(&request_body).await?;
            let mut buffer = String::new();
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
                buffer.push_str(
                    std::str::from_utf8(&bytes).context("llm stream returned invalid utf-8")?,
                );

                while let Some(index) = buffer.find("\n\n") {
                    let frame = buffer[..index].to_string();
                    buffer = buffer[index + 2..].to_string();
                    for payload in parse_sse_frame(&frame)? {
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
                                    if !try_send_stream_event(&sender, ChatEvent::Text(content))
                                        .await
                                    {
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
                                finish_reason = reason;
                            }
                        }
                    }
                }
            }

            let tool_calls = tool_accumulators
                .into_iter()
                .filter(|item| !item.name.is_empty())
                .map(ToolCallAccumulator::into_tool_call)
                .collect::<Vec<_>>();

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
                        if !try_send_stream_event(&sender, ChatEvent::Text(recovered_reply)).await {
                            return Ok(());
                        }
                    } else {
                        let detail = build_empty_stream_reply_error(&finish_reason, max_iterations);
                        tracing::warn!(finish_reason, "stream ended without visible reply");
                        let _ = try_send_stream_event(&sender, ChatEvent::Error { detail }).await;
                        return Ok(());
                    }
                }

                let output_files = extract_output_files(&self.repo_root, &full_reply);
                if !output_files.is_empty() {
                    if !try_send_stream_event(&sender, ChatEvent::OutputFiles(output_files)).await {
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
                );
                log_final_reply(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    &finish_reason,
                    &full_reply,
                );
                let _ = try_send_stream_event(&sender, ChatEvent::Done { finish_reason }).await;
                return Ok(());
            }

            let mut compress_requested = false;
            for tool_call in &tool_calls {
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
                        &mut skill_usages,
                    )
                    .await;
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
                messages.push(ChatMessage::tool(tool_call.id.clone(), result));
            }

            self.apply_todo_reminder(
                &mut messages,
                prepared.session.as_deref(),
                &tool_calls,
                &mut rounds_without_todo,
            );

            if compress_requested {
                messages =
                    auto_compact(&self.repo_root, &self.config, &self.llm_client, messages).await?;
            }
        }

        let mut final_finish_reason = "max_iterations".to_string();
        if full_reply.trim().is_empty() {
            if let Some(recovered_reply) = self
                .recover_missing_final_reply(&messages, &prepared.llm_overrides, "max_iterations")
                .await?
            {
                full_reply = recovered_reply.clone();
                final_finish_reason = "stop".to_string();
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
        );
        log_final_reply(
            &mode,
            prepared.session.as_deref(),
            prepared.user_id.as_deref(),
            &final_finish_reason,
            &full_reply,
        );
        Ok(())
    }

    fn prepare_request(&self, request: ChatRequest, mode: &ChatMode) -> Result<PreparedRequest> {
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
            .system
            .unwrap_or_else(|| self.build_system(request.user_id.as_deref()));
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
        })
    }

    pub fn preview_system_prompts(&self, user_id: Option<&str>) -> Result<SystemPromptPreview> {
        let normalized_user_id = user_id.map(str::trim).filter(|value| !value.is_empty());
        let stateless_prompt = self.build_system(None);
        let memory_base_prompt = self.build_system(normalized_user_id);
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

    fn build_system(&self, user_id: Option<&str>) -> String {
        let scope = if user_id.is_some() {
            SkillScope::Effective
        } else {
            SkillScope::Shared
        };
        let descriptions = self
            .skill_service
            .list_items(scope, user_id)
            .map(|items| self.skill_service.render_descriptions(&items))
            .unwrap_or_else(|_| "(no skills available)".to_string());

        let base = format!(
            "You are a coding agent at {}. Use task + worktree tools for multi-task work. MCP tools (prefixed with mcp_) may be available when the MCP server is reachable. IMPORTANT: All user-downloadable generated files (.docx/.xlsx/.csv/.md) must be written under /app/outputs/ inside the container. In this workspace that maps to {}/outputs/. Do not place downloadable deliverables in uploads, memory files, or other directories.",
            self.repo_root.display(),
            self.repo_root.display()
        );
        if descriptions == "(no skills available)" {
            base
        } else {
            format!("{base}\n\nSkills available (call load_skill to use):\n{descriptions}")
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

    async fn load_public_tools(&self, include_session_tools: bool) -> Vec<serde_json::Value> {
        let mut tools = static_public_tool_schemas(include_session_tools);
        match self.mcp_client.list_tool_schemas().await {
            Ok(mcp_tools) => tools.extend(mcp_tools),
            Err(error) => {
                tracing::warn!(?error, "failed to load mcp tools; continuing without them")
            }
        }
        tools
    }

    async fn dispatch_public_tool(
        &self,
        tool_call: &ToolCall,
        session: Option<&SessionContext>,
        user_id: Option<&str>,
        memory_mode: bool,
        skill_usages: &mut Vec<SkillUsage>,
    ) -> String {
        let arguments = serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
            .unwrap_or_else(|_| json!({}));
        let tool_name = tool_call.function.name.as_str();

        if tool_name.starts_with("mcp_") {
            return self.mcp_client.call_tool(tool_name, arguments).await;
        }

        if tool_name == "read_file" && should_use_mcp_file_reader(&arguments) {
            return self.read_file_via_mcp(&arguments).await;
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
            other => Ok(format!("Unknown tool: {other}")),
        };

        outcome.unwrap_or_else(|error| {
            tracing::warn!(tool = tool_name, ?error, "public tool call failed");
            format!("Error: {error}")
        })
    }

    async fn read_file_via_mcp(&self, arguments: &serde_json::Value) -> String {
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
            )
            .await
    }

    async fn run_subagent(&self, prompt: &str, agent_type: &str) -> String {
        let mut messages = build_subagent_initial_messages(prompt);
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
        recovery_messages.push(ChatMessage::user(build_missing_reply_recovery_prompt(
            finish_reason,
        )));

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
        if !matches!(mode, ChatMode::Memory) {
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
                Some("Noted background results.".to_string()),
                Vec::new(),
            ));
        }

        let inbox = session.read_inbox()?;
        if !inbox.is_empty() {
            messages.push(ChatMessage::user(format!(
                "<inbox>{}</inbox>",
                serde_json::to_string_pretty(&inbox).context("failed to encode inbox messages")?
            )));
            messages.push(ChatMessage::assistant(
                Some("Noted inbox messages.".to_string()),
                Vec::new(),
            ));
        }

        Ok(())
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
                "<reminder>Update your todos.</reminder>".to_string(),
            ));
            messages.push(ChatMessage::assistant(
                Some("Noted, will update todos.".to_string()),
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
    ) {
        if !matches!(mode, ChatMode::Memory) {
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
            tracing::info!(
                user_id = user_id.as_deref().unwrap_or("-"),
                updated_skills = updated_skills.len(),
                "background memory side effects finished"
            );
        });
    }
}

async fn try_send_stream_event(sender: &mpsc::Sender<ChatEvent>, event: ChatEvent) -> bool {
    sender.send(event).await.is_ok()
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

#[derive(Clone)]
struct PreparedRequest {
    messages: Vec<ChatMessage>,
    cleaned_history: Vec<HistoryEntry>,
    user_message: String,
    session: Option<Arc<SessionContext>>,
    user_id: Option<String>,
    llm_overrides: crate::domain::chat::models::LlmOverrides,
    prompt_overrides: AgentPromptOverrides,
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

fn build_subagent_initial_messages(prompt: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(
            "You are an isolated subagent. You do not inherit the parent agent's conversation history, session state, memory files, or loaded skills unless they are explicitly included in the task prompt or tool outputs.",
        ),
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

fn build_missing_reply_recovery_prompt(finish_reason: &str) -> String {
    format!(
        "<final-answer-required>\nThe previous assistant attempt ended without any user-visible answer (finish_reason: {finish_reason}). Based only on the conversation and tool results already available, provide the best possible final answer now. Do not call tools. If something remains incomplete, explain that clearly.\n</final-answer-required>"
    )
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

fn log_final_reply(
    mode: &ChatMode,
    session: Option<&SessionContext>,
    user_id: Option<&str>,
    finish_reason: &str,
    assistant_reply: &str,
) {
    let mode = match mode {
        ChatMode::Stateless => "stateless",
        ChatMode::Memory => "memory",
    };
    let session_id = session
        .map(|session| session.session_id.as_str())
        .unwrap_or("-");
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
        ChatMode, append_system_instruction, build_missing_reply_recovery_prompt,
        build_subagent_initial_messages, extract_output_files, log_final_reply, resolve_max_iterations,
        static_public_tool_schemas,
    };
    use crate::domain::chat::models::LlmOverrides;
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
    fn subagent_starts_with_isolated_context_only() {
        let messages = build_subagent_initial_messages("inspect README");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(
            messages[0]
                .content
                .as_deref()
                .unwrap_or_default()
                .contains("isolated subagent")
        );
        assert_eq!(messages[1].role, "user");
        assert_eq!(messages[1].content.as_deref(), Some("inspect README"));
    }

    #[test]
    fn recovery_prompt_forces_final_answer_without_tools() {
        let prompt = build_missing_reply_recovery_prompt("max_iterations");
        assert!(prompt.contains("without any user-visible answer"));
        assert!(prompt.contains("finish_reason: max_iterations"));
        assert!(prompt.contains("Do not call tools"));
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
        assert_eq!(PathBuf::from(&files[0].path), report.canonicalize().unwrap());
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
