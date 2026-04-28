use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub agent: AgentConfig,
    pub server: ServerConfig,
    pub mcp: McpConfig,
    pub logging: LoggingConfig,
    pub memory: MemoryConfig,
    pub skills: SkillsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            agent: AgentConfig::default(),
            server: ServerConfig::default(),
            mcp: McpConfig::default(),
            logging: LoggingConfig::default(),
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub model_id: String,
    pub max_tokens: u32,
    pub base_url: String,
    pub api_key: String,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_iterations: usize,
    pub subagent_max_iterations: usize,
    pub auto_compact_token_threshold: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model_id: "qwen2.5-72b-instruct".to_string(),
            max_tokens: 8_000,
            base_url: "http://localhost:8000/v1".to_string(),
            api_key: "EMPTY".to_string(),
            temperature: None,
            top_p: None,
            max_iterations: 8,
            subagent_max_iterations: 30,
            auto_compact_token_threshold: 60_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub reload: bool,
    pub cors: CorsConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            reload: true,
            cors: CorsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_credentials: bool,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origins: vec!["*".to_string()],
            allow_credentials: false,
            allow_methods: vec!["*".to_string()],
            allow_headers: vec!["*".to_string()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub base_url: String,
    pub timeout: u64,
    pub connect_timeout: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8444/mcp".to_string(),
            timeout: 60,
            connect_timeout: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub dir: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            dir: "logs".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub file_memory: FileMemoryConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            file_memory: FileMemoryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FileMemoryConfig {
    pub base_dir: String,
    pub user_max_chars: usize,
    pub memory_max_chars: usize,
    pub restructure_ratio: f32,
    pub update_max_tokens: u32,
}

impl Default for FileMemoryConfig {
    fn default() -> Self {
        Self {
            base_dir: ".user_memories".to_string(),
            user_max_chars: 1375,
            memory_max_chars: 2200,
            restructure_ratio: 0.5,
            update_max_tokens: 1800,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub learning: SkillLearningConfig,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            learning: SkillLearningConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SkillLearningConfig {
    pub enabled: bool,
    pub update_max_tokens: u32,
}

impl Default for SkillLearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            update_max_tokens: 2200,
        }
    }
}
