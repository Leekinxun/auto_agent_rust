use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use serde_json::json;
use tokio::sync::Mutex;

use crate::domain::memory::models::{UserMemorySnapshot, UserWorkspacePaths};
use crate::infra::fs::user_memory_store::FileMemoryStore;
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage};

#[derive(Clone)]
pub struct UserMemoryService {
    store: FileMemoryStore,
    locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
}

impl UserMemoryService {
    pub fn new(store: FileMemoryStore) -> Self {
        Self {
            store,
            locks: Arc::new(DashMap::new()),
        }
    }

    #[allow(dead_code)]
    pub fn ensure_workspace(&self, user_id: &str) -> Result<UserWorkspacePaths> {
        self.store.ensure_workspace(user_id)
    }

    pub fn load_snapshot(&self, user_id: &str) -> Result<UserMemorySnapshot> {
        self.store.load_snapshot(user_id)
    }

    pub fn build_user_memory_system(
        &self,
        base_system: &str,
        snapshot: Option<&UserMemorySnapshot>,
    ) -> String {
        let Some(snapshot) = snapshot else {
            return base_system.to_string();
        };

        let mut blocks = Vec::new();
        if !snapshot.user_md.is_empty() {
            blocks.push(format!(
                "<user_profile>\nLoaded from USER.md at session start. Treat as stable user guidance until the next session.\n{}\n</user_profile>",
                snapshot.user_md
            ));
        }
        if !snapshot.memory_md.is_empty() {
            blocks.push(format!(
                "<workspace_memory>\nLoaded from MEMORY.md at session start. Treat as reusable project context until the next session.\n{}\n</workspace_memory>",
                snapshot.memory_md
            ));
        }
        if blocks.is_empty() {
            return base_system.to_string();
        }
        format!("{base_system}\n\n{}", blocks.join("\n\n"))
    }

    pub async fn maintain_after_turn(
        &self,
        llm_client: &LlmClient,
        model: &str,
        max_tokens: u32,
        user_id: Option<&str>,
        user_message: &str,
        assistant_reply: &str,
    ) -> Result<()> {
        let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(());
        };

        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let paths = self.store.ensure_workspace(user_id)?;
        let current_user = self.store.read_user_md(user_id)?;
        let current_memory = self.store.read_memory_md(user_id)?;

        let user_limit = 1375usize;
        let memory_limit = 2200usize;
        let restructure_ratio = 0.5f32;
        let user_restructure =
            current_user.len() >= (user_limit as f32 * restructure_ratio) as usize;
        let memory_restructure =
            current_memory.len() >= (memory_limit as f32 * restructure_ratio) as usize;

        let prompt = format!(
            "You are a maintenance agent for two local markdown memory files of a coding assistant.\n\
Inspect the files with tools, decide whether they need updates, and directly modify them with the write tools.\n\n\
USER.md scope:\n\
- stable user preferences that would follow the user across projects\n\
- communication style\n\
- expectations\n\
- work habits\n\n\
MEMORY.md scope:\n\
- environment facts\n\
- tool quirks\n\
- project conventions\n\
- implementation decisions\n\
- learned experience\n\n\
Decision guide:\n\
- Put coding workflow rules, architecture choices, operational procedures, backend/frontend integration rules, file layout rules, and tool-usage constraints in MEMORY.md.\n\
- Put personal preferences about tone, collaboration style, and recurring user habits in USER.md.\n\
- If something could fit both files, prefer MEMORY.md unless it is clearly a portable personal preference across unrelated projects.\n\
- It is acceptable to update both files in one run when the latest turn contains both user-level and project-level durable information.\n\n\
Rules:\n\
- Keep only durable, reusable information likely to help future sessions.\n\
- Do not store one-off task details unless they imply a reusable convention.\n\
- Prefer short bullets or very short sections.\n\
- Only call a write tool when that file should actually change.\n\
- Do not return replacement file contents in normal text; use the write tools instead.\n\
- Hard limit for USER.md: {user_limit} characters.\n\
- Hard limit for MEMORY.md: {memory_limit} characters.\n\
- USER.md aggressive compacting: {}.\n\
- MEMORY.md aggressive compacting: {}.\n\
- USER.md path: {}\n\
- MEMORY.md path: {}\n\n\
Read both files first unless you already have enough context from prior tool results in this run.\n\n\
Current USER.md size: {} chars.\n\
Current MEMORY.md size: {} chars.\n\n\
<latest_turn>\nUser:\n{}\n\nAssistant:\n{}\n</latest_turn>\n\n\
When you are done, respond with a brief summary such as 'updated USER.md', 'updated MEMORY.md', 'updated both', or 'no changes'.",
            if user_restructure { "yes" } else { "no" },
            if memory_restructure { "yes" } else { "no" },
            paths.user_md.display(),
            paths.memory_md.display(),
            current_user.len(),
            current_memory.len(),
            clip_text(user_message, 5_000),
            clip_text(assistant_reply, 5_000)
        );

        let mut messages = vec![
            ChatMessage::system(
                "You maintain USER.md and MEMORY.md by using tools to inspect and directly update files. Never emit the full replacement file bodies in plain text.",
            ),
            ChatMessage::user(prompt),
        ];
        let mut read_state = HashSet::new();
        let tools = memory_update_tools();

        for _ in 0..8 {
            let response = llm_client
                .chat(&ChatCompletionRequest {
                    model: model.to_string(),
                    messages: messages.clone(),
                    tools: Some(tools.clone()),
                    stream: false,
                    temperature: None,
                    max_tokens: Some(max_tokens),
                    top_p: None,
                })
                .await?;
            let Some(choice) = response.choices.into_iter().next() else {
                break;
            };

            let assistant_message = choice.message;
            let tool_calls = assistant_message.tool_calls.clone();
            messages.push(assistant_message.into_chat_message());
            if tool_calls.is_empty() {
                break;
            }

            for tool_call in tool_calls {
                let arguments =
                    serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
                        .unwrap_or_else(|_| json!({}));
                let result = self.dispatch_memory_tool(
                    user_id,
                    &tool_call.function.name,
                    &arguments,
                    user_limit,
                    memory_limit,
                    &mut read_state,
                )?;
                messages.push(ChatMessage::tool(tool_call.id, result));
            }
        }

        Ok(())
    }

    fn dispatch_memory_tool(
        &self,
        user_id: &str,
        name: &str,
        arguments: &serde_json::Value,
        user_limit: usize,
        memory_limit: usize,
        read_state: &mut HashSet<&'static str>,
    ) -> Result<String> {
        match name {
            "read_user_md" => {
                read_state.insert("user");
                Ok(read_memory_file(
                    "USER.md",
                    &self.store.read_user_md(user_id)?,
                    user_limit,
                ))
            }
            "read_memory_md" => {
                read_state.insert("memory");
                Ok(read_memory_file(
                    "MEMORY.md",
                    &self.store.read_memory_md(user_id)?,
                    memory_limit,
                ))
            }
            "write_user_md" => {
                if !read_state.contains("user") {
                    return Ok("Error: Read USER.md first before writing it.".to_string());
                }
                write_memory_file(
                    &self.store,
                    user_id,
                    "USER.md",
                    arguments
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                    user_limit,
                    true,
                )
            }
            "write_memory_md" => {
                if !read_state.contains("memory") {
                    return Ok("Error: Read MEMORY.md first before writing it.".to_string());
                }
                write_memory_file(
                    &self.store,
                    user_id,
                    "MEMORY.md",
                    arguments
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default(),
                    memory_limit,
                    false,
                )
            }
            _ => Ok(format!("Error: Unknown tool {name}")),
        }
    }

    fn user_lock(&self, user_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(user_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

fn memory_update_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_user_md",
                "description": "Read the current contents of USER.md before deciding whether to update it.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_memory_md",
                "description": "Read the current contents of MEMORY.md before deciding whether to update it.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_user_md",
                "description": "Replace the full contents of USER.md with compact markdown.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "The complete new USER.md content." }
                    },
                    "required": ["content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_memory_md",
                "description": "Replace the full contents of MEMORY.md with compact markdown.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "The complete new MEMORY.md content." }
                    },
                    "required": ["content"]
                }
            }
        }),
    ]
}

fn read_memory_file(label: &str, current: &str, limit: usize) -> String {
    let header = format!("{label} ({}/{limit} chars)", current.len());
    if current.is_empty() {
        format!("{header}\n(empty)")
    } else {
        format!("{header}\n{current}")
    }
}

fn write_memory_file(
    store: &FileMemoryStore,
    user_id: &str,
    label: &str,
    content: &str,
    limit: usize,
    is_user: bool,
) -> Result<String> {
    let normalized = normalize_text(content);
    if normalized.len() > limit {
        return Ok(format!(
            "Error: {label} content too long ({} > {limit}). Rewrite it more compactly and call the write tool again.",
            normalized.len()
        ));
    }

    let current = if is_user {
        store.read_user_md(user_id)?
    } else {
        store.read_memory_md(user_id)?
    };
    if normalized == current {
        return Ok(format!(
            "{label} unchanged ({}/{limit} chars).",
            normalized.len()
        ));
    }

    if is_user {
        store.write_user_md(user_id, &normalized)?;
    } else {
        store.write_memory_md(user_id, &normalized)?;
    }
    Ok(format!(
        "{label} updated ({}/{limit} chars).",
        normalized.len()
    ))
}

fn normalize_text(text: &str) -> String {
    let mut compacted = Vec::new();
    let mut previous_blank = true;
    for line in text.replace("\r\n", "\n").split('\n') {
        let trimmed = line.trim_end();
        let blank = trimmed.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        compacted.push(trimmed.to_string());
        previous_blank = blank;
    }
    compacted.join("\n").trim().to_string()
}

fn clip_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let clipped = text.chars().take(limit).collect::<String>();
    format!("{}\n...[truncated]", clipped.trim_end())
}
