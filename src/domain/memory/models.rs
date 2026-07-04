use std::path::PathBuf;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct UserWorkspacePaths {
    pub root_dir: PathBuf,
    pub user_dir: PathBuf,
    pub user_md: PathBuf,
    pub memory_md: PathBuf,
    pub skills_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct UserMemorySnapshot {
    pub user_id: String,
    pub user_md: String,
    pub memory_md: String,
}

#[derive(Debug, Clone)]
pub struct UserMemoryResetResult {
    pub user_id: String,
    pub paths: UserWorkspacePaths,
    pub user_md_cleared: bool,
    pub memory_md_cleared: bool,
    pub private_skills_cleared: bool,
    pub private_skill_count: usize,
}
