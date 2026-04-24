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
    pub top_p: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub message: String,
    pub history: Vec<HistoryEntry>,
    pub system: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub files: Vec<UploadedFile>,
    pub llm_overrides: LlmOverrides,
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
