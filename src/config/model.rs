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
    pub prompts: MemoryPromptConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            file_memory: FileMemoryConfig::default(),
            prompts: MemoryPromptConfig::default(),
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
pub struct MemoryPromptConfig {
    pub maintenance_system: String,
    pub maintenance_user_template: String,
}

impl Default for MemoryPromptConfig {
    fn default() -> Self {
        Self {
            maintenance_system:
                "You maintain USER.md and MEMORY.md by using tools to inspect and directly update files. Never emit the full replacement file bodies in plain text."
                    .to_string(),
            maintenance_user_template:
                "You are a maintenance agent for two local markdown memory files of a coding assistant.\n\
Inspect the files with tools, decide whether they need updates, and directly modify them with the write tools.\n\n\
USER.md scope:\n\
- stable user preferences that would follow the user across projects\n\
- communication style\n\
- expectations\n\
- work habits\n\n\
MEMORY.md scope:\n\
- environment facts\n\
- tool quirks\n\
- project conventions\n\
- implementation decisions\n\
- learned experience\n\n\
Decision guide:\n\
- Put coding workflow rules, architecture choices, operational procedures, backend/frontend integration rules, file layout rules, and tool-usage constraints in MEMORY.md.\n\
- Put personal preferences about tone, collaboration style, and recurring user habits in USER.md.\n\
- If something could fit both files, prefer MEMORY.md unless it is clearly a portable personal preference across unrelated projects.\n\
- It is acceptable to update both files in one run when the latest turn contains both user-level and project-level durable information.\n\n\
Rules:\n\
- Keep only durable, reusable information likely to help future sessions.\n\
- Do not store one-off task details unless they imply a reusable convention.\n\
- Prefer short bullets or very short sections.\n\
- Only call a write tool when that file should actually change.\n\
- Do not return replacement file contents in normal text; use the write tools instead.\n\
- Hard limit for USER.md: {user_limit} characters.\n\
- Hard limit for MEMORY.md: {memory_limit} characters.\n\
- USER.md aggressive compacting: {user_restructure}.\n\
- MEMORY.md aggressive compacting: {memory_restructure}.\n\
- USER.md path: {user_md_path}\n\
- MEMORY.md path: {memory_md_path}\n\n\
Read both files first unless you already have enough context from prior tool results in this run.\n\n\
Current USER.md size: {current_user_len} chars.\n\
Current MEMORY.md size: {current_memory_len} chars.\n\n\
<latest_turn>\nUser:\n{user_message}\n\nAssistant:\n{assistant_reply}\n</latest_turn>\n\n\
When you are done, respond with a brief summary such as 'updated USER.md', 'updated MEMORY.md', 'updated both', or 'no changes'."
                    .to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub learning: SkillLearningConfig,
    pub prompts: SkillPromptConfig,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            learning: SkillLearningConfig::default(),
            prompts: SkillPromptConfig::default(),
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SkillPromptConfig {
    pub learning_system: String,
    pub learning_user_template: String,
}

impl Default for SkillPromptConfig {
    fn default() -> Self {
        Self {
            learning_system:
                "You improve private skills for future runs by using tools to inspect and directly update the file. Keep changes concise and durable, and never emit the full replacement body in plain text."
                    .to_string(),
            learning_user_template:
                "You are maintaining a user's private SKILL.md after a real assistant run.\n\
Inspect the current skill body with tools, decide whether it should change, and directly update the file with the write tool.\n\n\
Rules:\n\
- Preserve the main purpose of the skill.\n\
- Keep only durable, reusable guidance.\n\
- Do not include one-off outputs, timestamps, or transient details.\n\
- Prefer concise operational instructions.\n\
- If the latest turn adds no durable lesson, leave the file unchanged.\n\
- Do not return replacement markdown in normal text; use the write tool instead.\n\
Skill source for this turn: {source_scope}\n\n\
Private skill path: {skill_path}\n\
Private skill name: {skill_name}\n\
Current body size: {skill_body_len} chars\n\n\
<latest_turn>\n\
User:\n{user_message}\n\n\
Assistant:\n{assistant_reply}\n\
</latest_turn>\n\n\
Read the current skill body first unless you already have it from earlier tool results in this run.\n\
When you are done, respond briefly with 'updated skill' or 'no changes'."
                    .to_string(),
        }
    }
}
