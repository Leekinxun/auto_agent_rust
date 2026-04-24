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
