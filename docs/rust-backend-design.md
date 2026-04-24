# Rust 后端重构设计

## 1. 目标

本次 Rust 重构的目标不是改产品形态，而是把当前 Python 后端的核心能力稳定迁移过去，并保持前端可无感切换。

必须保留的行为：

- 继续兼容现有前端接口：
  - `POST /agent/run`
  - `POST /agent/stream`
  - `POST /agent/memory/run`
  - `POST /agent/memory/stream`
  - `GET /agent/memory`
  - `DELETE /agent/memory/session/{session_id}`
  - `GET/POST/PUT/DELETE /agent/skills...`
  - `GET /agent/download/{file_path}`
  - `GET /health`
  - SPA 静态前端托管
- 保留文件型记忆：
  - `.user_memories/<user>/USER.md`
  - `.user_memories/<user>/MEMORY.md`
- 保留公用和私有 skills 双层机制：
  - `skills/<folder>/SKILL.md`
  - `.user_memories/<user>/skills/<folder>/SKILL.md`
- 同名 skill 时，私有 skill 覆盖公用 skill
- 只有记忆模式才会：
  - 创建用户目录
  - 维护 `USER.md` / `MEMORY.md`
  - 学习并更新私有 skills
- 无痕模式不落盘、不复制公用 skill、不学习私有 skill

明确不做的事：

- 不再恢复 `mem0`、向量库或额外记忆数据库
- 不把 skills 编译进 Rust 二进制
- 不在后端持久化聊天历史，只保留请求级 history 和会话级运行态

## 2. 技术选型

推荐先做单二进制 crate，而不是一开始拆 workspace。原因很简单：当前项目体量还不需要为了“形式上的分层”增加太多维护成本。

建议栈：

- Web 框架：`axum`
- 异步运行时：`tokio`
- HTTP 客户端：`reqwest`
- JSON/YAML：`serde` `serde_json` `serde_yaml`
- SSE：`axum::response::sse`
- Multipart：`multer` 或 `axum-extra`
- 静态文件与 CORS：`tower-http`
- 日志：`tracing` `tracing-subscriber`
- 全局共享状态：`Arc` + `tokio::sync::{Mutex,RwLock}` + `dashmap`
- 路径与错误处理：`thiserror` `anyhow`
- 文件系统监听：
  - 第一阶段不必上 `notify`
  - 直接按请求读盘，确保 skill 随改随生效

## 3. 推荐目录结构

建议新建 `backend-rs/`，先并行开发，再切换容器入口，避免一次性替换 Python 服务。

```text
backend-rs/
  Cargo.toml
  src/
    main.rs
    app_state.rs
    config/
      mod.rs
      model.rs
      loader.rs
    api/
      mod.rs
      routes/
        agent.rs
        memory.rs
        skills.rs
        files.rs
        health.rs
        frontend.rs
      dto/
        agent.rs
        skills.rs
        settings.rs
      sse.rs
      errors.rs
    domain/
      mod.rs
      chat/
        orchestrator.rs
        tool_loop.rs
        models.rs
      session/
        service.rs
        models.rs
      memory/
        service.rs
        models.rs
        maintenance.rs
      skills/
        service.rs
        parser.rs
        learning.rs
        models.rs
      files/
        uploads.rs
        downloads.rs
      tools/
        dispatcher.rs
        registry.rs
      tasks/
        service.rs
      worktrees/
        service.rs
      events/
        service.rs
      mcp/
        service.rs
    infra/
      llm/
        client.rs
        stream.rs
        types.rs
      fs/
        user_memory_store.rs
        skill_store.rs
        static_store.rs
      session/
        in_memory_store.rs
    support/
      path_safety.rs
      text.rs
      token.rs
      sanitize.rs
```

## 4. 分层原则

### 4.1 API 层

只负责：

- 接收 HTTP 请求
- 参数校验
- 调用 service
- 返回 JSON 或 SSE

不负责：

- 拼接系统提示词
- 管理 skill 优先级
- 修改 `USER.md` / `MEMORY.md`
- 决定公用 skill 是否复制为私有 skill

### 4.2 Domain Service 层

负责业务规则，所有关键逻辑都收敛到这里：

- Chat 编排
- Memory 注入与写回
- Skills 合并与学习
- 会话状态隔离
- 工具调度

### 4.3 Infra 层

负责和外部世界打交道：

- LLM OpenAI 兼容接口
- MCP 服务
- 本地文件系统
- 内存态 Session Store

## 5. 核心模型

```rust
pub struct UserWorkspacePaths {
    pub root_dir: PathBuf,
    pub user_dir: PathBuf,
    pub user_md: PathBuf,
    pub memory_md: PathBuf,
    pub skills_dir: PathBuf,
}

pub struct UserMemorySnapshot {
    pub user_id: String,
    pub user_md: String,
    pub memory_md: String,
}

pub enum SkillScope {
    Shared,
    Private,
    Effective,
}

pub struct SkillMeta {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<String>,
    pub trigger: Option<String>,
}

pub struct SkillDocument {
    pub meta: SkillMeta,
    pub body: String,
    pub folder: String,
    pub path: PathBuf,
    pub scope: SkillScope,
}

pub struct SkillUsage {
    pub name: String,
    pub source_scope: SkillScope,
}

pub struct ChatRequest {
    pub message: String,
    pub history: Vec<ChatMessage>,
    pub system: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub llm_options: Option<LlmOptions>,
}
```

## 6. 关键服务设计

### 6.1 `ConfigService`

职责：

- 加载 `config/config.yaml`
- 环境变量覆盖
- 提供只读配置快照

建议保留现有配置路径和字段，避免前后端和部署脚本一起改。

建议配置结构继续兼容：

```yaml
agent:
  model_id: "minimax-m2.5"
  max_tokens: 50000
  base_url: "http://10.100.3.22:9800/v1"
  api_key: "EMPTY"

server:
  host: "0.0.0.0"
  port: 8080
  cors:
    allow_origins: ["*"]
    allow_credentials: false
    allow_methods: ["*"]
    allow_headers: ["*"]

mcp:
  base_url: "http://10.100.3.22:8444/mcp"

memory:
  file_memory:
    base_dir: ".user_memories"
    user_max_chars: 1375
    memory_max_chars: 2200
    restructure_ratio: 0.5
    update_max_tokens: 1800

skills:
  learning:
    enabled: true
    update_max_tokens: 2200
```

### 6.2 `SessionService`

职责：

- 按 `session_id` 管理运行态上下文
- 隔离 todo、background task、team bus、prompt memory snapshot

建议实现：

- `DashMap<String, Arc<SessionContext>>`
- `SessionContext` 内包含：
  - `todo_state`
  - `background_jobs`
  - `team_bus`
  - `prompt_memory_snapshots: HashMap<String, UserMemorySnapshot>`

注意：

- 会话状态是运行时内存态，不持久化到磁盘
- 用户刷新页面后丢失会话态是可接受的

### 6.3 `UserMemoryService`

职责：

- 计算用户目录
- 创建 `USER.md` / `MEMORY.md` / `skills/`
- 读取 session 启动时的 memory snapshot
- 构建系统提示词注入块
- 对话结束后调用“记忆维护 agent”直接写文件

必须保留的文件结构：

```text
.user_memories/
  <safe_user_id>/
    USER.md
    MEMORY.md
    skills/
```

关键 API：

```rust
pub trait UserMemoryStore {
    fn get_paths(&self, user_id: &str) -> Result<UserWorkspacePaths>;
    fn ensure_workspace(&self, user_id: &str) -> Result<UserWorkspacePaths>;
    fn load_snapshot(&self, user_id: &str) -> Result<UserMemorySnapshot>;
    fn read_user_md(&self, user_id: &str) -> Result<String>;
    fn read_memory_md(&self, user_id: &str) -> Result<String>;
    fn write_user_md(&self, user_id: &str, content: &str) -> Result<()>;
    fn write_memory_md(&self, user_id: &str, content: &str) -> Result<()>;
}
```

### 6.4 `MemoryMaintenanceService`

这是当前 Python `update_user_memory_files()` 的 Rust 落点。

职责：

- 请求结束后，调用一个“小型维护 agent”
- 通过工具直接：
  - 读 `USER.md`
  - 读 `MEMORY.md`
  - 写 `USER.md`
  - 写 `MEMORY.md`

这里要明确保留你之前要求的策略：

- 不让主对话模型在正文里返回 `<user_md>` 和 `<memory_md>`
- 由后处理维护 agent 直接判断是否需要修改文件
- 修改通过工具完成，而不是靠普通文本协议

建议接口：

```rust
pub async fn maintain_after_turn(
    &self,
    user_id: &str,
    user_message: &str,
    assistant_reply: &str,
) -> Result<MemoryMaintenanceResult>;
```

并发要求：

- 每个 `user_id` 使用独立 `tokio::sync::Mutex`
- 防止同一用户两个记忆请求同时改写 `USER.md` / `MEMORY.md`

### 6.5 `SkillService`

职责：

- 读取 shared/private/effective skills
- 解析 frontmatter
- CRUD
- 决定同名时的覆盖规则
- 为系统提示词提供精简描述
- 为 `load_skill(name)` 返回完整内容

保留的优先级规则：

- `effective = shared + private override`
- 同名时以私有 skill 为准

这里建议做一个非常明确的策略：

- shared skill 不做长期内存缓存
- private skill 每次按请求现读

原因：

- 你已经明确要“skills 目录可随时调整”
- 直接读盘最稳，不会出现 Rust 进程缓存导致界面改了但请求还拿旧内容

如果后面要优化性能，可做“按文件 mtime 的轻缓存”，但不是第一阶段必需。

关键接口：

```rust
pub trait SkillStore {
    fn list_shared(&self) -> Result<Vec<SkillDocument>>;
    fn list_private(&self, user_id: &str) -> Result<Vec<SkillDocument>>;
    fn list_effective(&self, user_id: Option<&str>) -> Result<Vec<SkillDocument>>;
    fn get_shared(&self, name: &str) -> Result<Option<SkillDocument>>;
    fn get_private(&self, user_id: &str, name: &str) -> Result<Option<SkillDocument>>;
    fn get_effective(&self, user_id: Option<&str>, name: &str) -> Result<Option<SkillDocument>>;
    fn save(&self, input: SaveSkillInput) -> Result<SkillDocument>;
    fn delete(&self, input: DeleteSkillInput) -> Result<SkillDocument>;
}
```

### 6.6 `SkillLearningService`

这是当前 Python `learn_from_usage()` 的 Rust 落点。

职责：

- 仅在记忆模式启用
- 仅在 `skills.learning.enabled = true` 时启用
- 收集本轮使用过的 skill
- 如果本轮用的是 shared skill：
  - 确保用户私有目录有一份同名 skill
  - 若没有，则复制 shared skill 到私有目录
- 然后通过维护 agent 直接更新该私有 skill 的正文

关键规则：

- 无痕模式完全跳过
- 共享 skill 被用户使用后，学习成果写入用户私有 skill
- 私有 skill 已存在时，直接在私有 skill 上继续优化

建议接口：

```rust
pub async fn learn_from_usage(
    &self,
    user_id: &str,
    usages: &[SkillUsage],
    user_message: &str,
    assistant_reply: &str,
) -> Result<Vec<SkillDocument>>;
```

并发要求：

- 同样按 `user_id` 加锁
- 防止同一用户并发复制或重写同一个 skill

### 6.7 `ChatOrchestrator`

这是整个系统的编排核心，对应当前 Python 的：

- `_prepare_memory_messages()`
- `_agent_loop()`
- `/agent/run`
- `/agent/stream`
- `/agent/memory/run`
- `/agent/memory/stream`

职责：

- 构建最终 system prompt
- 拼接 history
- 调用 LLM
- 执行工具循环
- 在记忆模式下调用 memory/skill 后处理
- 统一生成 sync 或 streaming 输出

建议拆成两个入口：

```rust
pub async fn run_once(&self, req: ChatRequest, mode: ChatMode) -> Result<ChatResponse>;
pub async fn stream_once(&self, req: ChatRequest, mode: ChatMode) -> Result<SseStream>;
```

其中：

```rust
pub enum ChatMode {
    Stateless,
    Memory,
}
```

## 7. 请求时序

### 7.1 无痕流式

```text
HTTP Handler
  -> ChatOrchestrator::stream_once(mode = Stateless)
    -> SessionService::get_or_create(session_id)
    -> SkillService::descriptions(user_id = None)
    -> LlmClient::stream_chat(...)
    -> ToolDispatcher loop
    -> SSE: text/tool_use/tool_result/done
```

特点：

- 不创建用户目录
- 不读取 `USER.md` / `MEMORY.md`
- 不学习私有 skill

### 7.2 记忆流式

```text
HTTP Handler
  -> ChatOrchestrator::stream_once(mode = Memory)
    -> SessionService::get_or_create(session_id)
    -> UserMemoryService::ensure_workspace(user_id)
    -> UserMemoryService::load_snapshot(user_id)
    -> SkillService::descriptions(user_id = Some(user_id))
    -> LlmClient::stream_chat(...)
    -> ToolDispatcher loop
    -> UserMemoryService::maintain_after_turn(...)
    -> SkillLearningService::learn_from_usage(...)
    -> SSE: skills_updated/done
```

特点：

- 会创建：
  - `USER.md`
  - `MEMORY.md`
  - `skills/`
- session 内只使用“本次会话开始时”的 snapshot
- 本轮写回不会反向污染本轮已加载的 prompt

## 8. Skills 动态加载策略

这是重构里最重要的点之一。

### 8.1 为什么不能把 skills 编译进 Rust

因为这样会破坏你现在想要的能力：

- 手动编辑 skill 文件后立即生效
- 前端 skills 管理界面增删改后立即生效
- 用户私有 skill 可在运行中持续进化

所以 Rust 后端必须继续把 skills 当成本地文件资源，而不是静态代码资源。

### 8.2 推荐实现

每次涉及 skills 的请求时：

- 读 shared skills 目录
- 如果有 `user_id`，再读用户私有 skills 目录
- 合并为 effective skills
- private 覆盖 shared 同名项

即：

```text
effective_skills = shared_skills
effective_skills.extend(private_skills_by_name)
```

### 8.3 文件格式

继续沿用：

```md
---
name: xxx
description: xxx
tags: a,b,c
trigger: xxx
---

# Skill Body
...
```

### 8.4 同名冲突规则

必须固定为：

- 系统提示词描述注入时：私有优先
- `load_skill(name)` 时：私有优先
- skills 列表页 `scope=effective` 时：私有优先

## 9. API 兼容设计

建议 Rust 第一阶段完全复刻现有 HTTP 合同，不先改前端。

### 9.1 Chat

- `POST /agent/run`
- `POST /agent/stream`
- `POST /agent/memory/run`
- `POST /agent/memory/stream`

表单字段保持兼容：

- `message`
- `history`
- `system`
- `session_id`
- `user_id`
- `agent_id`
- `model_id`
- `temperature`
- `max_tokens`
- `top_p`
- `files`

### 9.2 Memory

- `GET /agent/memory?user_id=...`
- `DELETE /agent/memory/session/{session_id}`

### 9.3 Skills

- `GET /agent/skills?scope=shared|private|effective&user_id=...`
- `POST /agent/skills`
- `PUT /agent/skills/{skill_name}`
- `DELETE /agent/skills/{skill_name}?scope=...&user_id=...`
- `POST /agent/skills/reload`

说明：

- 即使 Rust 版默认按请求读盘，`reload` 也可以先保留，作为兼容接口
- 实现上它可以只是返回当前重新读取后的结果

### 9.4 Files / Health / Frontend

- `GET /agent/download/{file_path}`
- `GET /health`
- `GET /`
- `GET /{frontend_path}`

## 10. LLM 与工具调用设计

### 10.1 `LlmClient`

需要统一支持：

- 非流式 chat completion
- 流式 chat completion
- tool calling

建议接口：

```rust
#[async_trait]
pub trait LlmClient {
    async fn chat(&self, req: ChatCompletionRequest) -> Result<ChatCompletionResponse>;
    async fn stream_chat(&self, req: ChatCompletionRequest) -> Result<LlmEventStream>;
}
```

### 10.2 工具调度

建议单独做 `ToolDispatcher`，不要把工具调用散在 route handler 里。

负责：

- 根据 tool name 分发到 handler
- 处理本地工具与 MCP 工具
- 记录 skill usage

技能加载工具建议保留：

- `load_skill(name)`

并在记忆模式时记录：

- 本轮到底用了哪个 skill
- 它来自 shared 还是 private

## 11. SSE 事件设计

建议沿用现有前端消费格式：

- `text`
- `tool_use`
- `tool_result`
- `files_uploaded`
- `output_files`
- `skills_updated`
- `done`
- `error`

Rust 里要注意两点：

- 上游模型一旦收到 chunk，就立刻往前端 flush，不要等拼完整句
- 对 `tool_calls` 也要支持增量拼接参数，避免“最后一下才冒出完整结果”

这部分如果做好，前端看到的模型输出就能更接近逐字流式。

## 12. 锁与并发

建议做三类锁：

### 12.1 用户记忆锁

- key: `user_id`
- 用途：保护 `USER.md` / `MEMORY.md`

### 12.2 用户私有 skill 学习锁

- key: `user_id`
- 用途：保护私有 skill 复制与更新

### 12.3 Session 运行态

- key: `session_id`
- 用途：隔离 todo / bg / team bus

建议实现：

```rust
DashMap<String, Arc<tokio::sync::Mutex<()>>>
```

## 13. 路径安全

Rust 版必须保留当前 Python 的安全边界。

必须做的检查：

- `user_id` 转安全目录名
- skill folder 禁止路径穿越
- `DELETE /agent/download/{file_path}` 不存在，这很好，继续不要提供删除接口
- 下载文件只能从允许目录查找
- 私有 skill 的目标路径必须位于该用户 `skills/` 根目录内

建议把这些逻辑收敛到：

- `support/path_safety.rs`
- `support/sanitize.rs`

## 14. 迁移顺序

推荐按下面顺序迁移，而不是一次性重写完再切换：

1. 先搭 Rust 壳子
   - `health`
   - CORS
   - 静态前端托管
   - 配置加载
2. 迁移 skills CRUD
   - 先把 `GET/POST/PUT/DELETE /agent/skills` 跑通
   - 验证 shared/private/effective 逻辑
3. 迁移无痕聊天
   - `/agent/run`
   - `/agent/stream`
4. 迁移记忆聊天
   - 用户目录创建
   - snapshot 注入
   - `USER.md` / `MEMORY.md` 写回
5. 迁移私有 skill 学习
   - shared -> private copy
   - 私有 skill 迭代优化
6. 迁移 task/worktree/event/mcp
7. 切换 Dockerfile / compose 到 Rust 服务
8. 下线 Python 入口

## 15. 最小可落地版本

如果希望尽快启动 Rust 迁移，建议第一版只做这些：

- `health`
- 静态前端托管
- skills CRUD
- `/agent/stream`
- `/agent/memory/stream`
- 文件型记忆
- 私有 skill 学习

先把最核心的交互链路打通，再补：

- `/agent/run`
- task/worktree/team/background
- 更完整的错误结构和观测指标

## 16. 建议的第一阶段验收标准

以下条件同时满足，Rust 版就算进入可替换状态：

- 前端不改代码即可直连 Rust 后端
- 无痕流式可正常对话
- 记忆流式会创建：
  - `USER.md`
  - `MEMORY.md`
  - 用户私有 `skills/`
- shared/private 同名 skill 时，实际注入的是私有 skill
- shared skill 在记忆模式下被使用后，会在私有目录复制一份并允许后续优化
- 无痕模式不会创建任何用户目录和私有 skill
- skills 管理界面不再出现“明明可用但状态显示失败”的问题

## 17. 最终建议

这次 Rust 重构不需要改变你的产品规则，只需要把当前后端重构成更清晰的几个核心服务：

- `ChatOrchestrator`
- `UserMemoryService`
- `MemoryMaintenanceService`
- `SkillService`
- `SkillLearningService`
- `SessionService`
- `LlmClient`

只要这几个边界定清楚，Rust 版就能既保留你现在的文件驱动能力，又把后续维护成本降下来。

下一步如果继续推进，最合适的是直接搭一版 `backend-rs` 骨架，把：

- `health`
- `skills CRUD`
- `memory workspace`

这三块先落地出来。
