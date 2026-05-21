pub mod apply;
pub mod approval;
pub mod decision;
pub mod draft;
pub mod signals;
pub mod snapshot;
pub mod trace;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SYSTEM_BASE_PATH: &str = "harness/system/base.md";
pub const SUBAGENT_SHARED_PATH: &str = "harness/subagents/shared.md";
pub const SUBAGENT_EXPLORE_PATH: &str = "harness/subagents/explore.md";
pub const SUBAGENT_GENERAL_PATH: &str = "harness/subagents/general-purpose.md";
pub const FINAL_ANSWER_RECOVERY_PATH: &str = "harness/middleware/final-answer-recovery.md";
pub const MEMORY_MAINTENANCE_SYSTEM_PATH: &str = "harness/memory/maintenance_system.md";
pub const MEMORY_MAINTENANCE_USER_TEMPLATE_PATH: &str =
    "harness/memory/maintenance_user_template.md";
pub const SKILL_LEARNING_SYSTEM_PATH: &str = "harness/skills/learning_system.md";
pub const SKILL_LEARNING_USER_TEMPLATE_PATH: &str = "harness/skills/learning_user_template.md";
pub const TOOL_DESCRIPTIONS_PATH: &str = "harness/tools/descriptions.json";
pub const MIDDLEWARE_MESSAGES_PATH: &str = "harness/middleware/messages.json";

const DEFAULT_SYSTEM_BASE_TEMPLATE: &str = "You are a coding agent at {repo_root}. Use task + worktree tools for multi-task work. MCP tools (prefixed with mcp_) may be available when the MCP server is reachable. IMPORTANT: All user-downloadable generated files (.docx/.xlsx/.csv/.md) must be written under /app/outputs/ inside the container. In this workspace that maps to {outputs_dir}. Do not place downloadable deliverables in uploads, memory files, or other directories.";

const DEFAULT_SUBAGENT_SHARED_PROMPT: &str = "You are an isolated subagent. You do not inherit the parent agent's conversation history, session state, memory files, or loaded skills unless they are explicitly included in the task prompt or tool outputs.";

const DEFAULT_SUBAGENT_EXPLORE_PROMPT: &str = "Stay read-only. Inspect relevant files carefully and return a grounded summary with concrete paths and concise evidence. Do not propose changes you did not inspect.";

const DEFAULT_SUBAGENT_GENERAL_PROMPT: &str = "You may use read_file, write_file, and edit_file when needed. Keep edits minimal and reversible, and summarize exactly what changed.";

const DEFAULT_FINAL_ANSWER_RECOVERY_TEMPLATE: &str = "<final-answer-required>\nThe previous assistant attempt ended without any user-visible answer (finish_reason: {finish_reason}). Based only on the conversation and tool results already available, provide the best possible final answer now. Do not call tools. If something remains incomplete, explain that clearly.\n</final-answer-required>";

const DEFAULT_BACKGROUND_RESULTS_ACK: &str = "Noted background results.";
const DEFAULT_INBOX_ACK: &str = "Noted inbox messages.";
const DEFAULT_STEERING_ACK: &str =
    "Noted steering update. Re-evaluating before running more tools.";
const DEFAULT_TODO_REMINDER_USER: &str = "<reminder>Update your todos.</reminder>";
const DEFAULT_TODO_REMINDER_ACK: &str = "Noted, will update todos.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSourceKind {
    Builtin,
    File,
    Request,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSource {
    pub kind: PromptSourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl PromptSource {
    pub fn builtin() -> Self {
        Self {
            kind: PromptSourceKind::Builtin,
            path: None,
        }
    }

    pub fn file(relative_path: &str) -> Self {
        Self {
            kind: PromptSourceKind::File,
            path: Some(relative_path.to_string()),
        }
    }

    pub fn request() -> Self {
        Self {
            kind: PromptSourceKind::Request,
            path: None,
        }
    }

    pub fn none() -> Self {
        Self {
            kind: PromptSourceKind::None,
            path: None,
        }
    }
}

#[derive(Debug, Clone)]
struct PromptAsset {
    content: String,
    source: PromptSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct MiddlewareMessages {
    pub background_results_ack: String,
    pub inbox_ack: String,
    pub steering_ack: String,
    pub todo_reminder_user: String,
    pub todo_reminder_ack: String,
}

impl Default for MiddlewareMessages {
    fn default() -> Self {
        Self {
            background_results_ack: DEFAULT_BACKGROUND_RESULTS_ACK.to_string(),
            inbox_ack: DEFAULT_INBOX_ACK.to_string(),
            steering_ack: DEFAULT_STEERING_ACK.to_string(),
            todo_reminder_user: DEFAULT_TODO_REMINDER_USER.to_string(),
            todo_reminder_ack: DEFAULT_TODO_REMINDER_ACK.to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MiddlewareMessagesOverride {
    background_results_ack: Option<String>,
    inbox_ack: Option<String>,
    steering_ack: Option<String>,
    todo_reminder_user: Option<String>,
    todo_reminder_ack: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HarnessAssets {
    system_base_template: PromptAsset,
    subagent_shared_prompt: PromptAsset,
    subagent_explore_prompt: PromptAsset,
    subagent_general_prompt: PromptAsset,
    final_answer_recovery_template: PromptAsset,
    tool_descriptions: HashMap<String, String>,
    tool_descriptions_source: PromptSource,
    middleware_messages: MiddlewareMessages,
    middleware_messages_source: PromptSource,
}

impl HarnessAssets {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let (tool_descriptions, tool_descriptions_source) =
            read_optional_tool_descriptions(repo_root)?;
        let (middleware_messages, middleware_messages_source) =
            read_optional_middleware_messages(repo_root)?;
        Ok(Self {
            system_base_template: read_optional_prompt(
                repo_root,
                SYSTEM_BASE_PATH,
                DEFAULT_SYSTEM_BASE_TEMPLATE,
            )?,
            subagent_shared_prompt: read_optional_prompt(
                repo_root,
                SUBAGENT_SHARED_PATH,
                DEFAULT_SUBAGENT_SHARED_PROMPT,
            )?,
            subagent_explore_prompt: read_optional_prompt(
                repo_root,
                SUBAGENT_EXPLORE_PATH,
                DEFAULT_SUBAGENT_EXPLORE_PROMPT,
            )?,
            subagent_general_prompt: read_optional_prompt(
                repo_root,
                SUBAGENT_GENERAL_PATH,
                DEFAULT_SUBAGENT_GENERAL_PROMPT,
            )?,
            final_answer_recovery_template: read_optional_prompt(
                repo_root,
                FINAL_ANSWER_RECOVERY_PATH,
                DEFAULT_FINAL_ANSWER_RECOVERY_TEMPLATE,
            )?,
            tool_descriptions,
            tool_descriptions_source,
            middleware_messages,
            middleware_messages_source,
        })
    }

    pub fn render_system_base(&self, repo_root: &Path) -> String {
        render_template(
            &self.system_base_template.content,
            &[
                ("repo_root", repo_root.display().to_string()),
                (
                    "outputs_dir",
                    repo_root.join("outputs").display().to_string(),
                ),
            ],
        )
    }

    pub fn render_subagent_system(&self, agent_type: &str) -> String {
        let role_specific = if agent_type.eq_ignore_ascii_case("explore") {
            self.subagent_explore_prompt.content.trim()
        } else {
            self.subagent_general_prompt.content.trim()
        };
        if role_specific.is_empty() {
            return self.subagent_shared_prompt.content.trim().to_string();
        }

        format!(
            "{}\n\n<role_specific_guidance>\n{}\n</role_specific_guidance>",
            self.subagent_shared_prompt.content.trim(),
            role_specific
        )
    }

    pub fn render_final_answer_recovery(&self, finish_reason: &str) -> String {
        render_template(
            &self.final_answer_recovery_template.content,
            &[("finish_reason", finish_reason.to_string())],
        )
    }

    pub fn system_base_source(&self) -> &PromptSource {
        &self.system_base_template.source
    }

    pub fn subagent_shared_source(&self) -> &PromptSource {
        &self.subagent_shared_prompt.source
    }

    pub fn subagent_explore_source(&self) -> &PromptSource {
        &self.subagent_explore_prompt.source
    }

    pub fn subagent_general_source(&self) -> &PromptSource {
        &self.subagent_general_prompt.source
    }

    pub fn final_answer_recovery_source(&self) -> &PromptSource {
        &self.final_answer_recovery_template.source
    }

    pub fn apply_tool_descriptions(&self, tools: &mut [Value]) {
        for tool in tools {
            let Some(function) = tool.get_mut("function") else {
                continue;
            };
            let Some(name) = function.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(description) = self.tool_descriptions.get(name) else {
                continue;
            };
            function["description"] = Value::String(description.clone());
        }
    }

    pub fn background_results_ack(&self) -> &str {
        self.middleware_messages.background_results_ack.trim()
    }

    pub fn inbox_ack(&self) -> &str {
        self.middleware_messages.inbox_ack.trim()
    }

    pub fn steering_ack(&self) -> &str {
        self.middleware_messages.steering_ack.trim()
    }

    pub fn todo_reminder_user(&self) -> &str {
        self.middleware_messages.todo_reminder_user.trim()
    }

    pub fn todo_reminder_ack(&self) -> &str {
        self.middleware_messages.todo_reminder_ack.trim()
    }

    pub fn system_base_template(&self) -> &str {
        &self.system_base_template.content
    }

    pub fn subagent_shared_prompt(&self) -> &str {
        &self.subagent_shared_prompt.content
    }

    pub fn subagent_explore_prompt(&self) -> &str {
        &self.subagent_explore_prompt.content
    }

    pub fn subagent_general_prompt(&self) -> &str {
        &self.subagent_general_prompt.content
    }

    pub fn final_answer_recovery_template(&self) -> &str {
        &self.final_answer_recovery_template.content
    }

    pub fn tool_descriptions(&self) -> &HashMap<String, String> {
        &self.tool_descriptions
    }

    pub fn tool_descriptions_source(&self) -> &PromptSource {
        &self.tool_descriptions_source
    }

    pub fn middleware_messages(&self) -> &MiddlewareMessages {
        &self.middleware_messages
    }

    pub fn middleware_messages_source(&self) -> &PromptSource {
        &self.middleware_messages_source
    }
}

pub fn resolve_repo_prompt_source(repo_root: &Path, relative_path: &str) -> PromptSource {
    if repo_root.join(relative_path).exists() {
        PromptSource::file(relative_path)
    } else {
        PromptSource::builtin()
    }
}

fn read_optional_prompt(
    repo_root: &Path,
    relative_path: &str,
    fallback: &str,
) -> Result<PromptAsset> {
    let path = repo_root.join(relative_path);
    if !path.exists() {
        return Ok(PromptAsset {
            content: fallback.to_string(),
            source: PromptSource::builtin(),
        });
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read harness prompt {}", path.display()))?;
    ensure!(
        !content.trim().is_empty(),
        "harness prompt {} is empty",
        path.display()
    );
    Ok(PromptAsset {
        content,
        source: PromptSource::file(relative_path),
    })
}

fn read_optional_tool_descriptions(
    repo_root: &Path,
) -> Result<(HashMap<String, String>, PromptSource)> {
    let path = repo_root.join(TOOL_DESCRIPTIONS_PATH);
    if !path.exists() {
        return Ok((HashMap::new(), PromptSource::none()));
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read tool descriptions {}", path.display()))?;
    let parsed = serde_json::from_str::<HashMap<String, String>>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok((
        parsed
            .into_iter()
            .filter_map(|(key, value)| {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if key.is_empty() || value.is_empty() {
                    None
                } else {
                    Some((key, value))
                }
            })
            .collect(),
        PromptSource::file(TOOL_DESCRIPTIONS_PATH),
    ))
}

fn read_optional_middleware_messages(
    repo_root: &Path,
) -> Result<(MiddlewareMessages, PromptSource)> {
    let path = repo_root.join(MIDDLEWARE_MESSAGES_PATH);
    if !path.exists() {
        return Ok((MiddlewareMessages::default(), PromptSource::builtin()));
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read middleware messages {}", path.display()))?;
    let overrides = serde_json::from_str::<MiddlewareMessagesOverride>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let mut messages = MiddlewareMessages::default();

    if let Some(value) = non_empty_trimmed(overrides.background_results_ack) {
        messages.background_results_ack = value;
    }
    if let Some(value) = non_empty_trimmed(overrides.inbox_ack) {
        messages.inbox_ack = value;
    }
    if let Some(value) = non_empty_trimmed(overrides.steering_ack) {
        messages.steering_ack = value;
    }
    if let Some(value) = non_empty_trimmed(overrides.todo_reminder_user) {
        messages.todo_reminder_user = value;
    }
    if let Some(value) = non_empty_trimmed(overrides.todo_reminder_ack) {
        messages.todo_reminder_ack = value;
    }

    Ok((messages, PromptSource::file(MIDDLEWARE_MESSAGES_PATH)))
}

fn render_template(template: &str, replacements: &[(&str, String)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in replacements {
        rendered = rendered.replace(&format!("{{{key}}}"), value);
    }
    rendered
}

fn non_empty_trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_BACKGROUND_RESULTS_ACK, HarnessAssets, MIDDLEWARE_MESSAGES_PATH,
        TOOL_DESCRIPTIONS_PATH,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRepo {
        root: PathBuf,
    }

    impl TestRepo {
        fn new() -> Self {
            let unique = format!(
                "auto-claude-harness-{}-{}",
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

    #[test]
    fn loads_custom_prompt_files_and_renders_placeholders() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("harness/system")).unwrap();
        fs::create_dir_all(repo.root.join("harness/middleware")).unwrap();
        fs::write(
            repo.root.join("harness/system/base.md"),
            "repo={repo_root}\noutputs={outputs_dir}",
        )
        .unwrap();
        fs::write(
            repo.root
                .join("harness/middleware/final-answer-recovery.md"),
            "recover {finish_reason}",
        )
        .unwrap();

        let harness = HarnessAssets::load(&repo.root).unwrap();
        let base = harness.render_system_base(&repo.root);
        assert!(base.contains(&format!("repo={}", repo.root.display())));
        assert!(base.contains(&format!("outputs={}", repo.root.join("outputs").display())));
        assert_eq!(
            harness.render_final_answer_recovery("max_iterations"),
            "recover max_iterations"
        );
    }

    #[test]
    fn composes_shared_and_role_specific_subagent_guidance() {
        let repo = TestRepo::new();
        let harness = HarnessAssets::load(&repo.root).unwrap();

        let explore = harness.render_subagent_system("Explore");
        let general = harness.render_subagent_system("general-purpose");

        assert!(explore.contains("isolated subagent"));
        assert!(explore.contains("Stay read-only"));
        assert!(general.contains("isolated subagent"));
        assert!(general.contains("write_file"));
    }

    #[test]
    fn applies_tool_description_overrides_from_json_file() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("harness/tools")).unwrap();
        fs::write(
            repo.root.join(TOOL_DESCRIPTIONS_PATH),
            r#"{
  "read_file": "Custom read description",
  "task": "Custom task description"
}"#,
        )
        .unwrap();

        let harness = HarnessAssets::load(&repo.root).unwrap();
        let mut tools = vec![
            serde_json::json!({
                "type": "function",
                "function": { "name": "read_file", "description": "orig" }
            }),
            serde_json::json!({
                "type": "function",
                "function": { "name": "task", "description": "orig" }
            }),
        ];
        harness.apply_tool_descriptions(&mut tools);

        assert_eq!(
            tools[0]["function"]["description"],
            serde_json::json!("Custom read description")
        );
        assert_eq!(
            tools[1]["function"]["description"],
            serde_json::json!("Custom task description")
        );
    }

    #[test]
    fn loads_middleware_message_overrides() {
        let repo = TestRepo::new();
        fs::create_dir_all(repo.root.join("harness/middleware")).unwrap();
        fs::write(
            repo.root.join(MIDDLEWARE_MESSAGES_PATH),
            r#"{
  "steering_ack": "Custom steering ack",
  "todo_reminder_user": "<reminder>Custom todos</reminder>",
  "todo_reminder_ack": "Custom todo ack"
}"#,
        )
        .unwrap();

        let harness = HarnessAssets::load(&repo.root).unwrap();

        assert_eq!(harness.steering_ack(), "Custom steering ack");
        assert_eq!(
            harness.todo_reminder_user(),
            "<reminder>Custom todos</reminder>"
        );
        assert_eq!(harness.todo_reminder_ack(), "Custom todo ack");
        assert_eq!(
            harness.background_results_ack(),
            DEFAULT_BACKGROUND_RESULTS_ACK
        );
    }
}
