use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use serde_json::json;
use tokio::sync::Mutex;

use crate::domain::chat::models::SkillUsage;
use crate::domain::skills::models::{
    DeleteSkillInput, InstallHubSkillInput, InstallHubSkillResult, SaveSkillInput, SkillDocument,
    SkillHubInstallRecord, SkillScope, UninstallHubSkillResult,
};
use crate::infra::fs::skill_store::FileSkillStore;
use crate::infra::llm::client::LlmClient;
use crate::infra::llm::types::{ChatCompletionRequest, ChatMessage};

#[derive(Clone)]
pub struct SkillService {
    store: FileSkillStore,
    locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
}

impl SkillService {
    pub fn new(store: FileSkillStore) -> Self {
        Self {
            store,
            locks: Arc::new(DashMap::new()),
        }
    }

    pub fn list_items(
        &self,
        scope: SkillScope,
        user_id: Option<&str>,
    ) -> Result<Vec<SkillDocument>> {
        self.store.list_items(scope, user_id)
    }

    pub fn save_skill(&self, input: SaveSkillInput) -> Result<SkillDocument> {
        self.store.save_skill(input)
    }

    pub fn delete_skill(&self, input: DeleteSkillInput) -> Result<SkillDocument> {
        self.store.delete_skill(input)
    }

    pub async fn install_hub_skill(
        &self,
        input: InstallHubSkillInput,
    ) -> Result<InstallHubSkillResult> {
        let lock = self.user_lock("__hub__");
        let _guard = lock.lock().await;
        self.store.install_hub_skill(input).await
    }

    pub fn list_hub_installations(&self) -> Result<Vec<SkillHubInstallRecord>> {
        self.store.list_hub_installations()
    }

    pub async fn uninstall_hub_skill(&self, name: &str) -> Result<UninstallHubSkillResult> {
        let lock = self.user_lock("__hub__");
        let _guard = lock.lock().await;
        self.store.uninstall_hub_skill(name)
    }

    pub fn get_resolved_skill(&self, name: &str, user_id: Option<&str>) -> Result<SkillDocument> {
        self.store.resolve_effective(name, user_id)
    }

    pub fn ensure_private_skill_copy(&self, user_id: &str, name: &str) -> Result<SkillDocument> {
        self.store.ensure_private_skill_copy(user_id, name)
    }

    pub fn get_private_skill(&self, user_id: &str, name: &str) -> Result<SkillDocument> {
        self.store.get_private_skill(user_id, name)
    }

    pub async fn learn_from_usage(
        &self,
        llm_client: &LlmClient,
        model: &str,
        max_tokens: u32,
        max_iterations: usize,
        user_id: Option<&str>,
        usages: &[SkillUsage],
        user_message: &str,
        assistant_reply: &str,
        system_prompt: &str,
        user_prompt_template: &str,
    ) -> Vec<SkillDocument> {
        let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return Vec::new();
        };

        let mut unique_usages = Vec::new();
        let mut seen = HashSet::new();
        for usage in usages {
            let name = usage.name.trim();
            if name.is_empty() || !seen.insert(name.to_string()) {
                continue;
            }
            unique_usages.push(SkillUsage {
                name: name.to_string(),
                scope: usage.scope,
            });
        }
        if unique_usages.is_empty() {
            return Vec::new();
        }

        let lock = self.user_lock(user_id);
        let _guard = lock.lock().await;

        let mut updated_skills = Vec::new();
        for usage in unique_usages {
            match self
                .learn_single_skill(
                    llm_client,
                    model,
                    max_tokens,
                    max_iterations,
                    user_id,
                    &usage,
                    user_message,
                    assistant_reply,
                    system_prompt,
                    user_prompt_template,
                )
                .await
            {
                Ok(Some(skill)) => updated_skills.push(skill),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        "private skill learning failed for {user_id}/{}: {error}",
                        usage.name
                    )
                }
            }
        }

        updated_skills
    }

    pub fn render_descriptions(&self, items: &[SkillDocument]) -> String {
        if items.is_empty() {
            return "(no skills available)".to_string();
        }

        items
            .iter()
            .map(|item| {
                let mut line = format!(
                    "  - {}: {}",
                    item.name,
                    if item.description.is_empty() {
                        "No description"
                    } else {
                        item.description.as_str()
                    }
                );
                if !item.tags.is_empty() {
                    line.push_str(&format!(" [tags: {}]", item.tags));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn learn_single_skill(
        &self,
        llm_client: &LlmClient,
        model: &str,
        max_tokens: u32,
        max_iterations: usize,
        user_id: &str,
        usage: &SkillUsage,
        user_message: &str,
        assistant_reply: &str,
        system_prompt: &str,
        user_prompt_template: &str,
    ) -> Result<Option<SkillDocument>> {
        let private_exists = self.get_private_skill(user_id, &usage.name).is_ok();
        let mut created_private_copy = false;
        if usage.scope == SkillScope::Shared {
            self.ensure_private_skill_copy(user_id, &usage.name)?;
            created_private_copy = !private_exists;
        }

        let body_changed = self
            .rewrite_private_skill_body(
                llm_client,
                model,
                max_tokens,
                max_iterations,
                user_id,
                &usage.name,
                usage.scope,
                user_message,
                assistant_reply,
                system_prompt,
                user_prompt_template,
            )
            .await?;

        if created_private_copy || body_changed {
            return self.get_private_skill(user_id, &usage.name).map(Some);
        }

        Ok(None)
    }

    async fn rewrite_private_skill_body(
        &self,
        llm_client: &LlmClient,
        model: &str,
        max_tokens: u32,
        max_iterations: usize,
        user_id: &str,
        skill_name: &str,
        source_scope: SkillScope,
        user_message: &str,
        assistant_reply: &str,
        system_prompt: &str,
        user_prompt_template: &str,
    ) -> Result<bool> {
        let mut skill = self.get_private_skill(user_id, skill_name)?;
        let original_body = skill.body.trim().to_string();
        let prompt = render_private_skill_learning_prompt(
            user_prompt_template,
            source_scope.as_str(),
            &skill.path,
            &skill.name,
            skill.body.len(),
            &clip_text(user_message, 5_000),
            &clip_text(assistant_reply, 7_000),
        );

        let mut messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(prompt),
        ];
        let mut read_state = HashSet::new();
        let tools = private_skill_update_tools();

        for _ in 0..max_iterations {
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
                let result = self.dispatch_private_skill_update_tool(
                    user_id,
                    &mut skill,
                    &tool_call.function.name,
                    &arguments,
                    &mut read_state,
                )?;
                messages.push(ChatMessage::tool(tool_call.id, result));
            }
        }

        Ok(skill.body.trim() != original_body)
    }

    fn dispatch_private_skill_update_tool(
        &self,
        user_id: &str,
        skill: &mut SkillDocument,
        name: &str,
        arguments: &serde_json::Value,
        read_state: &mut HashSet<&'static str>,
    ) -> Result<String> {
        match name {
            "read_private_skill_body" => {
                read_state.insert("body");
                Ok(read_private_skill_body(skill))
            }
            "write_private_skill_body" => {
                if !read_state.contains("body") {
                    return Ok("Error: Read the private skill body before writing it.".to_string());
                }
                let content = arguments
                    .get("content")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let normalized = normalize_text(content);
                if normalized.is_empty() {
                    return Ok("Error: Skill body cannot be empty.".to_string());
                }
                if normalized == skill.body.trim() {
                    return Ok(format!("SKILL.md unchanged ({} chars).", normalized.len()));
                }

                *skill =
                    self.store
                        .rewrite_private_skill_body(user_id, &skill.name, &normalized)?;
                Ok(format!("SKILL.md updated ({} chars).", skill.body.len()))
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

fn private_skill_update_tools() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_private_skill_body",
                "description": "Read the current markdown body of the private SKILL.md before deciding whether to update it.",
                "parameters": { "type": "object", "properties": {} }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_private_skill_body",
                "description": "Replace the markdown body of the private SKILL.md while preserving its frontmatter metadata.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "description": "The complete new markdown body for this skill." }
                    },
                    "required": ["content"]
                }
            }
        }),
    ]
}

fn read_private_skill_body(skill: &SkillDocument) -> String {
    let body = skill.body.trim();
    let header = format!("SKILL.md body ({} chars)", body.len());
    if body.is_empty() {
        format!("{header}\n(empty)")
    } else {
        format!("{header}\n{body}")
    }
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

fn render_private_skill_learning_prompt(
    template: &str,
    source_scope: &str,
    skill_path: &str,
    skill_name: &str,
    skill_body_len: usize,
    user_message: &str,
    assistant_reply: &str,
) -> String {
    template
        .replace("{source_scope}", source_scope)
        .replace("{skill_path}", skill_path)
        .replace("{skill_name}", skill_name)
        .replace("{skill_body_len}", &skill_body_len.to_string())
        .replace("{user_message}", user_message)
        .replace("{assistant_reply}", assistant_reply)
}
