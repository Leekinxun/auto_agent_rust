use serde::Serialize;

use crate::domain::chat::models::{HistoryEntry, OutputFile};
use crate::domain::skills::models::SkillDocument;

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub reply: String,
    pub history: Vec<HistoryEntry>,
    pub output_files: Vec<OutputFile>,
}

#[derive(Debug, Serialize)]
pub struct MemoryAgentResponse {
    pub reply: String,
    pub history: Vec<HistoryEntry>,
    pub skills_updated: Vec<SkillDocument>,
    pub output_files: Vec<OutputFile>,
}
