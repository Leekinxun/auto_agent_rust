# Auto Claude Code Rust React

这是一个从原项目中拆出来的纯 `Rust + React + TypeScript` 版本，不再包含 Python 后端。

## 目录

- `src/`: Rust 后端
- `frontend/`: React + TypeScript 前端
- `config/`: 服务配置
- `skills/`: 公共 skills
- `docs/`: 重构设计文档

## 接口文档

- Agent 接口文档（流式 + 非流式）：[`docs/agent-streaming-api.md`](docs/agent-streaming-api.md)

## 学习文档

- 后端学习手册（Rust + 项目入门）：[`docs/backend-learning-manual.md`](docs/backend-learning-manual.md)

## 本地启动

### 1. 构建前端静态资源

```bash
cd frontend
npm ci
npm run build
```

### 2. 启动 Rust 后端

```bash
cargo run
```

默认读取 `config/config.yaml`，当前默认端口是 `18000`。

当前会自动尝试加载仓库根目录 `.env`，便于本地开发时直接覆盖关键环境变量；如果 shell 中已显式导出同名环境变量，则仍以 shell 环境为准。

> `frontend` 的构建产物会输出到 `static/frontend/`，Rust 后端会直接托管该目录。
>
> 后端日志会同时输出到 stdout 和仓库根目录 `logs/`，按日期分文件保存，默认文件名形如 `backend-2026-04-28.log`。

## Docker 启动

推荐优先使用 Compose：

```bash
cp .env.example .env
docker compose up -d --build
```

默认通过 `http://localhost:18000` 访问。

如果不用 Compose，也可以直接使用 `docker run`：

```bash
cp .env.example .env
docker build -t auto-claude-code-rust-react:latest .

docker run -d \
  --name auto-claude-code-rust-react \
  --restart unless-stopped \
  --env-file .env \
  -p 18000:18000 \
  -v "$(pwd)/config:/app/config:ro" \
  -v "$(pwd)/skills:/app/skills" \
  -v "$(pwd)/logs:/app/logs" \
  -v "$(pwd)/.user_memories:/app/.user_memories" \
  -v "$(pwd)/uploads:/app/uploads" \
  -v "$(pwd)/outputs:/app/outputs" \
  -v "$(pwd)/.tasks:/app/.tasks" \
  -v "$(pwd)/.worktrees:/app/.worktrees" \
  -v "$(pwd)/.sessions:/app/.sessions" \
  -v "$(pwd)/.transcripts:/app/.transcripts" \
  auto-claude-code-rust-react:latest
```

上面的示例按默认端口 `18000` 启动；如果你修改了 `.env` 里的 `SERVER_PORT`，记得同步调整 `-p <宿主机端口>:<容器端口>`。

更完整的配置来源、环境变量覆盖、持久化目录与部署注意事项，见 [`docs/runtime-guide.md`](docs/runtime-guide.md)。

## 前端模式说明

- 当前前端只暴露两个聊天入口：
  - 无痕流式：`/chat/stream`
  - 记忆流式：`/chat/memory-stream`
- 旧的同步路径：
  - `/chat/normal`
  - `/chat/memory-run`
  目前仅保留为重定向兼容入口，不再作为独立页面模式。

## AHE / Harness Engineering（自进化 Harness）

当前仓库已经加入一套 **repo-native、人工可控、可审计、可回滚** 的 AHE-lite（Agentic Harness Engineering）能力，用来观察并逐步改进 agent 的 harness，而不是在运行时做黑盒自动变异。

### Harness 目录

以下 surface 以文件形式保存在仓库根目录 `harness/` 下：

- `harness/system/base.md`
- `harness/subagents/shared.md`
- `harness/subagents/explore.md`
- `harness/subagents/general-purpose.md`
- `harness/middleware/final-answer-recovery.md`
- `harness/middleware/messages.json`
- `harness/memory/*.md`
- `harness/skills/*.md`
- `harness/tools/descriptions.json`

这些文件会覆盖或补充运行时 prompt / tool 描述，使 harness 的调整可以通过 Git 管理、审阅、回滚与复现。

### 前端观察入口

前端新增 `Harness 观测` 页面：

- 路径：`/harness`
- 可查看：
  - snapshot（当前生效 surfaces）
  - signals（近期 trace 聚合信号）
  - traces（每次 run / stream 的结构化观测）
  - decision drafts / decisions
  - approvals（审批记录）

### 人工演进流程

当前 Harness 变更默认走 **人工审阅链路**：

1. 根据 recent traces 生成 draft
2. 人工保存为 decision
3. 在前端编辑目标 surface 内容
4. 生成 preview diff
5. 填写审批人 / 审批备注
6. 手动 apply 并热更新运行时
7. 如有必要，可按 approval 记录执行 rollback

这条链路的目标是：**先观察，再预览，再审批，再写入**。

### 关键约束：无痕模式不自进化

这是当前实现中的硬约束：

- `stateless / 无痕`：**不允许自进化**
- `memory / 记忆`：允许受控的 self-evolution 相关流程

也就是说，AHE 的观测与人工 apply 能力可以在前端看到，但 **无痕模式不会自动修改 harness**。

### 数据落盘位置

Harness 相关运行记录保存在 `.omx/` 下：

- traces：`.omx/traces/harness/*.json`
- decisions：`.omx/decisions/harness/*.json`
- approvals：`.omx/approvals/harness/*.json`
- 共享前端设置：`.omx/state/frontend-settings.json`

### 主要接口

- `GET /agent/harness/snapshot`
- `GET /agent/harness/signals`
- `GET /agent/harness/traces`
- `GET /agent/harness/drafts`
- `GET /agent/harness/decisions`
- `GET /agent/harness/approvals`
- `POST /agent/harness/preview-apply`
- `POST /agent/harness/apply`
- `POST /agent/harness/rollback`

### 设置页共享策略

设置页现在分为两类：

#### 共享并持久化到服务端

这些设置会写入 `.omx/state/frontend-settings.json`，其他用户打开前端时也会自动生效：

- 品牌标题 / 副标题
- MCP 相关共享配置
- agent prompt append
- model / temperature / top_p / max_tokens / max_iterations
- memory / skill 维护 prompt override

#### 仅当前浏览器本地保存

以下设置仍保存在浏览器 `localStorage`，不会同步给其他用户：

- API Base URL
- 默认用户 ID
- 聊天记录
- skills 草稿编辑状态

这样可以避免多人共用同一个后端入口或同一个记忆身份。

## 当前迁移状态

- 已完成：
  - `GET /health`
  - `GET /agent/memory`
  - `GET /agent/tasks`
  - `GET /agent/worktrees`
  - `GET /agent/events`
  - `DELETE /agent/memory/session/{session_id}`
  - `GET/POST/PUT/DELETE /agent/skills`
  - `POST /agent/skills/reload`
  - `GET /agent/download/{file_path}`
  - `POST /agent/run`
  - `POST /agent/stream`
  - `POST /agent/memory/run`
  - `POST /agent/memory/stream`
  - 静态前端托管
- 当前已具备：
  - 多轮对话
  - SSE 流式输出
  - 工作区安全的 `read_file` / `write_file` / `edit_file` 工具
  - MCP 动态工具注入与 `mcp_*` 工具调用
  - `.docx/.xlsx/.xls/.csv` 通过 MCP 支持读取
  - `task` 同步 subagent 工具
  - task / worktree / event 基础能力与对应 agent 工具
  - session 级 `TodoWrite` / background / inbox 状态隔离与 LRU 驱逐
  - session 级 teammate 管理：`spawn_teammate` / `list_teammates` / `shutdown_request` / `plan_approval`
  - lead inbox 读取时会登记 `plan_request`，使 `plan_approval` 形成可用闭环
  - 前端已消费流式 `files_uploaded` / `skills_updated` / `error` / `done` 事件
  - 前端会将工具调用、工具结果、上传文件、skill 更新、完成状态、错误等过程事件做可读化展示，而不再仅显示原始 JSON
  - prompt memory snapshot session cache
  - 后台结果注入、inbox 注入、todo reminder
  - `microcompact` / `auto_compact` / `compress` 上下文压缩链路
  - `load_skill` 工具调用
  - 记忆模式下的 `USER.md` / `MEMORY.md` 注入与维护
  - 记忆模式下使用公用 skill 时创建私有副本
- 已补齐的兼容与测试：
  - `agent_id` 兼容：`POST /agent/memory/run`、`POST /agent/memory/stream`、`GET /agent/memory` 现在都接受 `agent_id`，并在缺少 `user_id` 时回退使用它
  - MCP 降级测试：MCP 服务不可用时不会阻塞普通 chat
  - task / worktree / event 持久化回归测试
  - teammate / plan approval 回归测试（spawn / list / persistence / shutdown request / approval loop）
  - Python→Rust 工具面兼容守卫测试（静态工具集合对表，不含动态 MCP）
  - 前端 Vitest 测试脚手架与基础测试：
    - 前端仅暴露两种流式聊天模式
    - 旧同步路径重定向兼容
    - 聊天滚动按钮文案 / 未读计数辅助逻辑
  - 关键 API smoke tests：`/health`、`/agent/run`、`/agent/stream`、`/agent/memory`、`/agent/memory/run`、`/agent/tasks`、`/agent/worktrees`、`/agent/events`、skills CRUD、download，以及 `task` subagent 与 session teammate 工具链路
  - 当前 `cargo test`：36 项通过
  - 当前 `frontend npm test`：7 项通过
  - 当前 `frontend npm run build`：通过

## 已知差异（当前有意保留）

- 未暴露通用 `bash` agent 工具；当前以 `read_file` / `write_file` / `edit_file`、background、worktree 等能力为主，保持更收敛的执行面。
- worktree 相关能力依赖当前目录本身是 git repo；在非 git 仓库中会返回受控错误，而不是崩溃。
- 前端层面当前只提供无痕流式与记忆流式两种聊天工作区；同步接口仍由后端保留，但不再单独暴露为前端入口。

## 运行时补充说明

- 配置默认来自 `config/config.yaml`，环境变量可覆盖关键字段。
- 新增支持的环境变量包括：
  - `AGENT_MODEL_ID` / `MODEL_ID`
  - `AGENT_BASE_URL` / `ANTHROPIC_BASE_URL`
  - `AGENT_API_KEY`
  - `AGENT_MAX_TOKENS`
  - `AGENT_TEMPERATURE`
  - `AGENT_TOP_P`
  - `MCP_BASE_URL`
  - `MCP_TIMEOUT`
  - `MCP_CONNECT_TIMEOUT`
  - `LOG_DIR`
- 空字符串环境变量会被忽略；数值型覆盖如果非法会在启动阶段直接报错，避免静默使用错误配置。
- Docker Compose 已补充 `config`、`skills`、`logs`、`.tasks`、`.worktrees`、`.sessions`、`.transcripts` 等挂载，便于持久化运行态数据。
- 用户文件记忆默认保存在：
  - 宿主机：`.user_memories/<用户目录>/USER.md`
  - 宿主机：`.user_memories/<用户目录>/MEMORY.md`
  - 容器内：`/app/.user_memories/<用户目录>/USER.md`
  - 容器内：`/app/.user_memories/<用户目录>/MEMORY.md`
- 默认根目录来自 `config/config.yaml` 中的 `memory.file_memory.base_dir: ".user_memories"`。
- `<用户目录>` 通常就是 `user_id`；如果 `user_id` 包含特殊字符、过长或不适合直接作为目录名，系统会自动做安全转换后再落盘。

更多细节见 [`docs/runtime-guide.md`](docs/runtime-guide.md)。
