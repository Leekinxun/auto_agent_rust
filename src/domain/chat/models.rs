use serde::Serialize;

use crate::domain::skills::models::{SkillDocument, SkillScope};

#[derive(Debug, Clone)]
pub enum ChatMode {
    Stateless,
    Memory,
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

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub message: String,
    pub history: Vec<HistoryEntry>,
    pub system: Option<String>,
    pub system_append: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub files: Vec<UploadedFile>,
    pub llm_overrides: LlmOverrides,
    pub prompt_overrides: AgentPromptOverrides,
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
pub struct OutputFile {
    pub name: String,
    pub path: String,
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
    ToolUse { name: String, arguments: String },
    ToolResult { tool: String, output: String },
    FilesUploaded(Vec<UploadedFile>),
    OutputFiles(Vec<OutputFile>),
    SkillsUpdated(Vec<SkillDocument>),
    Done { finish_reason: String },
    Error { detail: String },
}
