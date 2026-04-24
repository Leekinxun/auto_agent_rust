use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::model::FileMemoryConfig;
use crate::domain::memory::models::{UserMemorySnapshot, UserWorkspacePaths};
use crate::support::sanitize::safe_user_dir_name;

const USER_FILENAME: &str = "USER.md";
const MEMORY_FILENAME: &str = "MEMORY.md";

#[derive(Clone)]
pub struct FileMemoryStore {
    repo_root: PathBuf,
    config: FileMemoryConfig,
}

impl FileMemoryStore {
    pub fn new(repo_root: PathBuf, config: FileMemoryConfig) -> Self {
        Self { repo_root, config }
    }

    pub fn root_dir(&self) -> PathBuf {
        let configured = PathBuf::from(&self.config.base_dir);
        if configured.is_absolute() {
            configured
        } else {
            self.repo_root.join(configured)
        }
    }

    pub fn get_paths(&self, user_id: &str) -> UserWorkspacePaths {
        let root_dir = self.root_dir();
        let user_dir = root_dir.join(safe_user_dir_name(user_id));
        UserWorkspacePaths {
            root_dir,
            user_dir: user_dir.clone(),
            user_md: user_dir.join(USER_FILENAME),
            memory_md: user_dir.join(MEMORY_FILENAME),
            skills_dir: user_dir.join("skills"),
        }
    }

    pub fn ensure_workspace(&self, user_id: &str) -> Result<UserWorkspacePaths> {
        let paths = self.get_paths(user_id);
        std::fs::create_dir_all(&paths.skills_dir)
            .with_context(|| format!("failed to create {}", paths.skills_dir.display()))?;
        self.ensure_file(&paths.user_md)?;
        self.ensure_file(&paths.memory_md)?;
        Ok(paths)
    }

    pub fn load_snapshot(&self, user_id: &str) -> Result<UserMemorySnapshot> {
        let paths = self.ensure_workspace(user_id)?;
        Ok(UserMemorySnapshot {
            user_id: user_id.to_string(),
            user_md: read_trimmed(&paths.user_md)?,
            memory_md: read_trimmed(&paths.memory_md)?,
        })
    }

    pub fn read_user_md(&self, user_id: &str) -> Result<String> {
        let paths = self.ensure_workspace(user_id)?;
        read_trimmed(&paths.user_md)
    }

    pub fn read_memory_md(&self, user_id: &str) -> Result<String> {
        let paths = self.ensure_workspace(user_id)?;
        read_trimmed(&paths.memory_md)
    }

    pub fn write_user_md(&self, user_id: &str, content: &str) -> Result<()> {
        let paths = self.ensure_workspace(user_id)?;
        std::fs::write(&paths.user_md, content)
            .with_context(|| format!("failed to write {}", paths.user_md.display()))
    }

    pub fn write_memory_md(&self, user_id: &str, content: &str) -> Result<()> {
        let paths = self.ensure_workspace(user_id)?;
        std::fs::write(&paths.memory_md, content)
            .with_context(|| format!("failed to write {}", paths.memory_md.display()))
    }

    fn ensure_file(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            std::fs::write(path, "")
                .with_context(|| format!("failed to create {}", path.display()))?;
        }
        Ok(())
    }
}

fn read_trimmed(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .trim()
        .to_string())
}
