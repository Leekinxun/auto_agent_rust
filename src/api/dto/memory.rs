use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct MemoryQuery {
    pub user_id: Option<String>,
    pub agent_id: Option<String>,
}

impl MemoryQuery {
    pub fn effective_user_id(&self) -> Option<&str> {
        self.user_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.agent_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
    }
}

#[derive(Debug, Serialize)]
pub struct MemoryResponse {
    pub user_id: Option<String>,
    pub user_md: String,
    pub memory_md: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SessionDeleteResponse {
    pub success: bool,
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct UserMemoryResetResponse {
    pub success: bool,
    pub user_id: String,
    pub user_md_cleared: bool,
    pub memory_md_cleared: bool,
    pub private_skills_cleared: bool,
    pub private_skill_count: usize,
    pub cached_session_snapshots_cleared: usize,
    pub mcp_state: String,
    pub user_md_path: String,
    pub memory_md_path: String,
    pub private_skills_path: String,
}
