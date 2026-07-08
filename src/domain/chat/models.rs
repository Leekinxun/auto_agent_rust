use serde::Serialize;

use crate::domain::hitl::policy::{HitlDefaultAction, HitlPolicyRule};

use crate::domain::skills::models::{SkillDocument, SkillScope};
use crate::infra::llm::types::TokenUsage;
use crate::infra::mcp::client::McpServerPreview;

#[derive(Debug, Clone)]
pub enum ChatMode {
    Stateless,
    Memory,
}

impl ChatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stateless => "stateless",
            Self::Memory => "memory",
        }
    }

    pub fn allows_self_evolution(&self) -> bool {
        matches!(self, Self::Memory)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadedFile {
    pub original_name: String,
    pub saved_path: String,
    pub size: usize,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LlmOverrides {
    pub model_id: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub max_iterations: Option<usize>,
    pub top_p: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentPromptOverrides {
    pub memory_maintenance_system: Option<String>,
    pub memory_maintenance_user_template: Option<String>,
    pub skill_learning_system: Option<String>,
    pub skill_learning_user_template: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpExposureMode {
    Eager,
    Lazy,
    Disabled,
}

#[derive(Debug, Clone, Default)]
pub struct McpOverrides {
    pub config_path: Option<String>,
    pub base_urls: Vec<String>,
    pub disabled_urls: Vec<String>,
    pub lazy_urls: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

impl McpOverrides {
    pub fn exposure_mode_for_endpoint(&self, endpoint: &str) -> McpExposureMode {
        let normalized = normalize_mcp_endpoint(endpoint);
        if self
            .disabled_urls
            .iter()
            .any(|item| normalize_mcp_endpoint(item) == normalized)
        {
            return McpExposureMode::Disabled;
        }
        if self
            .lazy_urls
            .iter()
            .any(|item| normalize_mcp_endpoint(item) == normalized)
        {
            return McpExposureMode::Lazy;
        }
        McpExposureMode::Eager
    }

    pub fn has_lazy_endpoints(&self) -> bool {
        self.lazy_urls
            .iter()
            .any(|item| self.exposure_mode_for_endpoint(item) == McpExposureMode::Lazy)
    }
}

#[derive(Debug, Clone)]
pub struct BuiltinToolOverrides {
    pub enabled: bool,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
}

impl Default for BuiltinToolOverrides {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_tools: Vec::new(),
            denied_tools: Vec::new(),
        }
    }
}

impl BuiltinToolOverrides {
    pub fn allows(&self, name: &str) -> bool {
        let normalized = normalize_builtin_tool_name(name);
        if normalized.is_empty() || !is_builtin_tool(normalized) {
            return false;
        }
        if !self.enabled {
            return false;
        }
        if self
            .denied_tools
            .iter()
            .any(|item| normalize_builtin_tool_name(item) == normalized)
        {
            return false;
        }
        self.allowed_tools.is_empty()
            || self
                .allowed_tools
                .iter()
                .any(|item| normalize_builtin_tool_name(item) == normalized)
    }
}

pub const BUILTIN_TOOL_NAMES: &[&str] = &[
    "TodoWrite",
    "background_run",
    "broadcast",
    "check_background",
    "claim_task",
    "compress",
    "edit_file",
    "idle",
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
];

pub fn is_builtin_tool(name: &str) -> bool {
    let normalized = normalize_builtin_tool_name(name);
    BUILTIN_TOOL_NAMES.iter().any(|item| *item == normalized)
}

pub fn is_builtin_file_tool(name: &str) -> bool {
    matches!(
        normalize_builtin_tool_name(name),
        "read_file" | "write_file" | "edit_file"
    )
}

pub fn normalize_builtin_tool_list(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .map(|item| normalize_builtin_tool_name(&item).to_string())
        .filter(|item| !item.is_empty())
        .filter(|item| is_builtin_tool(item))
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

fn normalize_builtin_tool_name(name: &str) -> &str {
    name.trim()
}

#[derive(Debug, Clone)]
pub struct HitlOverrides {
    pub enabled: bool,
    pub default_action: HitlDefaultAction,
    pub timeout_seconds: u64,
    pub rules: Vec<HitlPolicyRule>,
}

impl Default for HitlOverrides {
    fn default() -> Self {
        Self {
            enabled: false,
            default_action: HitlDefaultAction::Auto,
            timeout_seconds: 300,
            rules: Vec::new(),
        }
    }
}

pub struct ChatRequest {
    pub message: String,
    pub history: Vec<HistoryEntry>,
    pub system: Option<String>,
    pub system_override: Option<String>,
    pub system_append: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub files: Vec<UploadedFile>,
    pub llm_overrides: LlmOverrides,
    pub prompt_overrides: AgentPromptOverrides,
    pub mcp_overrides: McpOverrides,
    pub builtin_tool_overrides: BuiltinToolOverrides,
    pub skill_permissions: SkillPermissions,
    pub hitl_overrides: HitlOverrides,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemPromptPreview {
    pub stateless_prompt: String,
    pub memory_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentPromptSettingsPreview {
    pub memory_maintenance_system: String,
    pub memory_maintenance_user_template: String,
    pub skill_learning_system: String,
    pub skill_learning_user_template: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpSettingsPreview {
    pub servers: Vec<McpServerPreviewDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerPreviewDto {
    pub endpoint: String,
    pub endpoint_key: String,
    pub mode: McpExposureMode,
    pub ok: bool,
    pub tool_count: usize,
    pub tools: Vec<McpToolPreviewDto>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpToolPreviewDto {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillPermissions {
    pub allowed_skills: Vec<String>,
    pub denied_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputFile {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SteeringSubmission {
    pub status: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResult {
    pub reply: String,
    pub history: Vec<HistoryEntry>,
    pub output_files: Vec<OutputFile>,
    pub skills_updated: Vec<SkillDocument>,
    pub token_usage: Option<TokenUsageReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageReport {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub context_window: Option<u32>,
    pub used_percent: Option<f32>,
}

impl TokenUsageReport {
    pub fn from_usage(usage: TokenUsage, context_window: Option<u32>) -> Option<Self> {
        if usage.prompt_tokens == 0 && usage.completion_tokens == 0 && usage.total_tokens == 0 {
            return None;
        }

        let total_tokens = if usage.total_tokens == 0 {
            usage.prompt_tokens.saturating_add(usage.completion_tokens)
        } else {
            usage.total_tokens
        };
        let used_percent = context_window
            .filter(|value| *value > 0)
            .map(|value| ((total_tokens as f64 / value as f64) * 100.0) as f32);

        Some(Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens,
            context_window,
            used_percent,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SkillUsage {
    pub name: String,
    pub scope: SkillScope,
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    Text(String),
    ToolUse {
        name: String,
        arguments: String,
    },
    ToolResult {
        tool: String,
        output: String,
    },
    Steering {
        message: String,
        skipped_tools: Vec<String>,
    },
    ApprovalRequired {
        approval_id: String,
        kind: String,
        title: String,
        summary: String,
        risk_level: String,
        tool_name: Option<String>,
        display_name: Option<String>,
        arguments: serde_json::Value,
    },
    ApprovalResolved {
        approval_id: String,
        status: String,
    },
    FilesUploaded(Vec<UploadedFile>),
    OutputFiles(Vec<OutputFile>),
    SkillsUpdated(Vec<SkillDocument>),
    TokenUsage(TokenUsageReport),
    Done {
        finish_reason: String,
    },
    Error {
        detail: String,
    },
}

impl From<McpServerPreview> for McpServerPreviewDto {
    fn from(value: McpServerPreview) -> Self {
        Self {
            endpoint: value.endpoint,
            endpoint_key: value.endpoint_key,
            mode: value.mode,
            ok: value.ok,
            tool_count: value.tool_count,
            tools: value
                .tools
                .into_iter()
                .map(|tool| McpToolPreviewDto {
                    name: tool.name,
                    description: tool.description,
                })
                .collect(),
            error: value.error,
        }
    }
}

fn normalize_mcp_endpoint(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::ChatMode;

    #[test]
    fn only_memory_mode_allows_self_evolution() {
        assert!(!ChatMode::Stateless.allows_self_evolution());
        assert!(ChatMode::Memory.allows_self_evolution());
    }
}
