use serde::Serialize;

use crate::domain::skills::models::{SkillDocument, SkillScope};
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
    FilesUploaded(Vec<UploadedFile>),
    OutputFiles(Vec<OutputFile>),
    SkillsUpdated(Vec<SkillDocument>),
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
