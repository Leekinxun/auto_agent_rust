use serde::Serialize;

use crate::domain::chat::models::{
    HistoryEntry, OutputFile, SteeringSubmission, TokenUsageReport,
};
use crate::domain::skills::models::SkillDocument;

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub reply: String,
    pub history: Vec<HistoryEntry>,
    pub output_files: Vec<OutputFile>,
    pub token_usage: Option<TokenUsageReport>,
}

#[derive(Debug, Serialize)]
pub struct MemoryAgentResponse {
    pub reply: String,
    pub history: Vec<HistoryEntry>,
    pub skills_updated: Vec<SkillDocument>,
    pub output_files: Vec<OutputFile>,
    pub token_usage: Option<TokenUsageReport>,
}

#[derive(Debug, Serialize)]
pub struct SteeringResponse {
    pub status: String,
    pub session_id: String,
}

impl From<SteeringSubmission> for SteeringResponse {
    fn from(value: SteeringSubmission) -> Self {
        Self {
            status: value.status,
            session_id: value.session_id,
        }
    }
}
