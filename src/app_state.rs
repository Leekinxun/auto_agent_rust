use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;

use crate::config::loader::load_config;
use crate::config::model::AppConfig;
use crate::domain::chat::orchestrator::ChatOrchestrator;
use crate::domain::events::service::EventService;
use crate::domain::harness::HarnessAssets;
use crate::domain::memory::service::UserMemoryService;
use crate::domain::session::service::SessionService;
use crate::domain::skills::service::SkillService;
use crate::domain::tasks::service::TaskService;
use crate::domain::worktree::service::WorktreeService;
use crate::infra::fs::skill_store::FileSkillStore;
use crate::infra::fs::user_memory_store::FileMemoryStore;
use crate::infra::llm::client::LlmClient;
use crate::infra::mcp::client::McpClient;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: AppConfig,
    pub session_service: SessionService,
    pub memory_service: UserMemoryService,
    pub skill_service: SkillService,
    pub event_service: EventService,
    pub task_service: TaskService,
    pub worktree_service: WorktreeService,
    pub chat_orchestrator: RwLock<ChatOrchestrator>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: AppConfig) -> Result<SharedState> {
        let memory_store =
            FileMemoryStore::new(repo_root.clone(), config.memory.file_memory.clone());
        let skill_store = FileSkillStore::new(repo_root.join("skills"), memory_store.clone());
        let memory_service = UserMemoryService::new(memory_store);
        let skill_service = SkillService::new(skill_store);
        let event_service = EventService::new(repo_root.clone())?;
        let task_service = TaskService::new(repo_root.clone())?;
        let worktree_service = WorktreeService::new(
            repo_root.clone(),
            task_service.clone(),
            event_service.clone(),
        )?;
        let session_service = SessionService::new(repo_root.clone());
        let llm_client = LlmClient::new(&config)?;
        let mcp_client = McpClient::new(&config)?;
        let harness = HarnessAssets::load(&repo_root, &config)?;
        let chat_orchestrator = ChatOrchestrator::new(
            repo_root.clone(),
            config.clone(),
            harness.clone(),
            llm_client.clone(),
            mcp_client,
            memory_service.clone(),
            skill_service.clone(),
            task_service.clone(),
            worktree_service.clone(),
            session_service.clone(),
        );

        Ok(Arc::new(Self {
            repo_root,
            config,
            session_service,
            memory_service,
            skill_service,
            event_service,
            task_service,
            worktree_service,
            chat_orchestrator: RwLock::new(chat_orchestrator),
        }))
    }

    pub fn chat_orchestrator_clone(&self) -> ChatOrchestrator {
        self.chat_orchestrator
            .read()
            .expect("chat orchestrator lock poisoned")
            .clone()
    }

    pub fn reload_chat_orchestrator(&self) -> Result<()> {
        let config_path = self.repo_root.join("config").join("config.yaml");
        let config = if config_path.exists() {
            load_config(&self.repo_root)?
        } else {
            self.config.clone()
        };
        let harness = HarnessAssets::load(&self.repo_root, &config)?;
        let llm_client = LlmClient::new(&config)?;
        let mcp_client = McpClient::new(&config)?;
        let chat_orchestrator = ChatOrchestrator::new(
            self.repo_root.clone(),
            config,
            harness,
            llm_client,
            mcp_client,
            self.memory_service.clone(),
            self.skill_service.clone(),
            self.task_service.clone(),
            self.worktree_service.clone(),
            self.session_service.clone(),
        );
        *self
            .chat_orchestrator
            .write()
            .expect("chat orchestrator lock poisoned") = chat_orchestrator;
        Ok(())
    }
}
