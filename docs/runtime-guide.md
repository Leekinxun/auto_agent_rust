# 运行与部署说明

本文补充 Rust + React 重构版的运行时约定，重点说明配置来源、环境变量覆盖、持久化目录以及 Docker 部署注意事项。

## 1. 本地运行

### 1.1 构建前端

```bash
cd frontend
npm ci
npm run build
```

前端产物会输出到：

```text
static/frontend/
```

Rust 后端会直接托管该目录。

### 1.2 启动后端

```bash
cargo run
```

默认读取：

```text
config/config.yaml
```

默认监听：

```text
0.0.0.0:18000
```

当前 `cargo run` 会自动尝试读取仓库根目录的 `.env`。

加载规则：

- `.env` 适合作为本地开发默认值
- 如果 shell 中已经显式导出了同名环境变量，则以 shell 环境为准
- 如果仓库根目录不存在 `.env`，启动不会报错

### 1.3 前端当前暴露的聊天模式

当前前端只保留两个聊天工作区：

- 无痕流式：`/chat/stream`
- 记忆流式：`/chat/memory-stream`

以下历史路径只保留重定向兼容，不再作为独立前端模式维护：

- `/chat/normal`
- `/chat/memory-run`

## 2. 配置来源与优先级

配置来源分两层：

1. `config/config.yaml`
2. 环境变量覆盖

环境变量优先级规则：

- `AGENT_MODEL_ID` 优先于 `MODEL_ID`
- `AGENT_BASE_URL` 优先于 `ANTHROPIC_BASE_URL`
- 其余字段按自身环境变量覆盖
- 空字符串会被忽略，不会把 YAML 中的值覆盖成空
- 非法数值会在启动时直接报错并终止启动

## 3. 当前支持的环境变量覆盖

### 3.1 Agent / LLM

| 环境变量 | 覆盖字段 | 说明 |
| --- | --- | --- |
| `AGENT_MODEL_ID` | `agent.model_id` | 新命名，优先级高于 `MODEL_ID` |
| `MODEL_ID` | `agent.model_id` | 兼容 Python 版本保留 |
| `AGENT_BASE_URL` | `agent.base_url` | 新命名，优先级高于 `ANTHROPIC_BASE_URL` |
| `ANTHROPIC_BASE_URL` | `agent.base_url` | 兼容旧命名保留 |
| `AGENT_API_KEY` | `agent.api_key` | OpenAI 兼容接口或代理可用 |
| `AGENT_MAX_TOKENS` | `agent.max_tokens` | 必须大于 0 |
| `AGENT_TEMPERATURE` | `agent.temperature` | 必须在 `0 ~ 2` |
| `AGENT_TOP_P` | `agent.top_p` | 必须在 `0 < top_p <= 1` |

### 3.2 Server / CORS

| 环境变量 | 覆盖字段 |
| --- | --- |
| `SERVER_HOST` | `server.host` |
| `SERVER_PORT` | `server.port` |
| `CORS_ALLOW_ORIGINS` | `server.cors.allow_origins` |
| `CORS_ALLOW_CREDENTIALS` | `server.cors.allow_credentials` |
| `CORS_ALLOW_METHODS` | `server.cors.allow_methods` |
| `CORS_ALLOW_HEADERS` | `server.cors.allow_headers` |

`CORS_ALLOW_*` 中的列表字段使用逗号分隔，例如：

```bash
export CORS_ALLOW_ORIGINS="http://localhost:5173,http://127.0.0.1:5173"
```

### 3.3 MCP

| 环境变量 | 覆盖字段 | 说明 |
| --- | --- | --- |
| `MCP_BASE_URL` | `mcp.base_url` | MCP 服务地址 |
| `MCP_TIMEOUT` | `mcp.timeout` | 请求超时秒数，必须大于 0 |
| `MCP_CONNECT_TIMEOUT` | `mcp.connect_timeout` | 连接超时秒数，必须大于 0 |

## 4. 持久化目录

以下目录建议在部署时持久化：

| 目录 | 用途 |
| --- | --- |
| `skills/` | 公共 skills（共享 skill CRUD） |
| `.user_memories/` | 用户文件记忆：`USER.md` / `MEMORY.md` / 私有 skills |
| `uploads/` | 上传文件暂存 |
| `outputs/` | 生成文件输出 |
| `.tasks/` | 任务持久化 |
| `.worktrees/` | worktree 索引、事件与工作目录 |
| `.sessions/` | session inbox / teammate 状态 |
| `.transcripts/` | 压缩/归档 transcript |

## 5. Docker Compose

推荐使用仓库根目录的 `docker-compose.yml`：

```bash
cp .env.example .env
docker compose up -d --build
```

当前 compose 已挂载：

- `config`
- `skills`
- `.user_memories`
- `uploads`
- `outputs`
- `.tasks`
- `.worktrees`
- `.sessions`
- `.transcripts`

这样容器重启后，绝大部分运行态数据都能保留。

## 6. Docker Run 示例

如果不用 Compose，可手动挂载：

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

## 7. Worktree 能力注意事项

worktree 相关能力依赖当前工作目录本身是一个真实 git 仓库。

这意味着：

- 如果你直接在当前这个“非 git 目录”里运行，worktree 工具会返回受控错误
- 如果你在 Docker 镜像内只使用 `COPY` 进去的代码，而没有挂载 `.git` 或完整仓库，也同样无法创建 git worktree

如果你希望在容器中启用 worktree，请保证容器内 `/app` 对应的是一个真实 git checkout，或额外挂载 `.git` 元数据。

## 8. 现阶段建议

如果你的目标是继续做 Python → Rust 的功能收尾，建议优先保持以下约束：

1. 不回退当前已通过的 API smoke tests
2. 保持 `agent_id` / `user_id` 兼容语义
3. 继续把“不支持”的能力表现为受控错误，而不是 panic
4. 先补配置、部署与运行时文档，再做纯 UI 美化
