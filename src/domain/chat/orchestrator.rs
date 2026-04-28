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
    TOKEN_THRESHOLD, auto_compact, estimate_tokens, microcompact, public_compaction_tool_schemas,
};
use crate::domain::chat::models::{
    ChatEvent, ChatMode, ChatRequest, ChatResult, HistoryEntry, OutputFile, SkillUsage,
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
        let mut messages = prepared.messages;
        let tools = self.load_public_tools(prepared.session.is_some()).await;
        let mut reply = String::new();
        let mut finish_reason = "stop".to_string();
        let mut rounds_without_todo = 0usize;

        for _ in 0..8 {
            microcompact(&mut messages);
            if estimate_tokens(&messages) > TOKEN_THRESHOLD {
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
        let mut messages = prepared.messages;
        let tools = self.load_public_tools(prepared.session.is_some()).await;
        let mut full_reply = String::new();
        let mut rounds_without_todo = 0usize;

        for _ in 0..8 {
            microcompact(&mut messages);
            if estimate_tokens(&messages) > TOKEN_THRESHOLD {
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

            while let Some(chunk) = stream.next().await {
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
                                    let _ = sender.send(ChatEvent::Text(content)).await;
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
                let output_files = extract_output_files(&self.repo_root, &full_reply);
                if !output_files.is_empty() {
                    let _ = sender.send(ChatEvent::OutputFiles(output_files)).await;
                }
                let skills_updated = self
                    .finalize_memory_side_effects(
                        &mode,
                        prepared.session.as_deref(),
                        prepared.user_id.as_deref(),
                        &prepared.user_message,
                        &full_reply,
                        &skill_usages,
                    )
                    .await;
                if !skills_updated.is_empty() {
                    let _ = sender.send(ChatEvent::SkillsUpdated(skills_updated)).await;
                }
                log_final_reply(
                    &mode,
                    prepared.session.as_deref(),
                    prepared.user_id.as_deref(),
                    &finish_reason,
                    &full_reply,
                );
                let _ = sender.send(ChatEvent::Done { finish_reason }).await;
                return Ok(());
            }

            let mut compress_requested = false;
            for tool_call in &tool_calls {
                let _ = sender
                    .send(ChatEvent::ToolUse {
                        name: tool_call.function.name.clone(),
                        arguments: tool_call.function.arguments.clone(),
                    })
                    .await;
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
                let _ = sender
                    .send(ChatEvent::ToolResult {
                        tool: tool_call.function.name.clone(),
                        output: preview,
                    })
                    .await;
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

        let _ = sender
            .send(ChatEvent::Done {
                finish_reason: "length".to_string(),
            })
            .await;
        log_final_reply(
            &mode,
            prepared.session.as_deref(),
            prepared.user_id.as_deref(),
            "length",
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
        let base_system = request
            .system
            .unwrap_or_else(|| self.build_system(request.user_id.as_deref()));

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
        })
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
            "You are a coding agent at {}. Use task + worktree tools for multi-task work. MCP tools (prefixed with mcp_) may be available when the MCP server is reachable. IMPORTANT: Save all generated output files (.docx/.xlsx/.csv/.md) to {}/outputs/ directory.",
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
        let mut messages = vec![ChatMessage::user(prompt.to_string())];
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

        for _ in 0..30 {
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

    async fn finalize_memory_side_effects(
        &self,
        mode: &ChatMode,
        session: Option<&SessionContext>,
        user_id: Option<&str>,
        user_message: &str,
        assistant_reply: &str,
        skill_usages: &[SkillUsage],
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
                user_id,
                user_message,
                assistant_reply,
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
                Some(user_id),
                skill_usages,
                user_message,
                assistant_reply,
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
        let canonical = resolved.display().to_string();
        if !seen.insert(canonical.clone()) {
            continue;
        }
        files.push(OutputFile {
            name: resolved
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
    use super::{ChatMode, log_final_reply, static_public_tool_schemas};
    use std::collections::HashSet;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

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
