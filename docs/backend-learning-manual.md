# 后端学习手册：从 0 开始学习本项目与 Rust

这份手册面向**刚接触 Rust**、同时也要理解本项目后端实现的学生。

目标不是一下子读完所有源码，而是：

1. 先理解 Rust 后端项目的基本结构
2. 再理解这个项目的一次请求是怎么跑起来的
3. 最后能自己修改、调试、扩展一个接口或一个模块

---

## 1. 你将学到什么

学完这份手册后，你应该能够：

- 知道 Rust 后端项目通常由哪些部分组成
- 看懂 `main.rs` 如何启动一个 Web 服务
- 看懂 Axum 路由是如何组织的
- 知道本项目的 `api / domain / infra / config / support` 是怎么分层的
- 理解一次聊天请求从 HTTP 到 LLM 再到响应返回的完整链路
- 看懂 Rust 中常见的 `struct`、`enum`、`Result`、`Option`、`Arc`、`async/await`
- 能在本项目里完成一些入门级修改

---

## 2. 学习本项目之前，先建立两个意识

### 2.1 这个项目本质上是什么？

这是一个 **Rust 后端 + React 前端** 的智能 Agent 系统。

后端主要负责：

- 接收前端或其他客户端发来的 HTTP 请求
- 解析聊天参数、历史消息、上传文件
- 组织 prompt
- 调用大模型（LLM）
- 在需要时调用工具（文件工具、任务工具、worktree 工具、MCP 工具等）
- 管理 session、记忆、skills
- 返回非流式 JSON 或流式 SSE 响应

### 2.2 学习这个项目，不要一上来就啃所有代码

推荐顺序是：

1. 先看程序怎么启动
2. 再看路由怎么分发
3. 再看状态是怎么初始化的
4. 再看聊天主链路
5. 最后再看 memory / skills / tasks / worktree 这些扩展能力

这和学习一座城市一样：

- `main.rs` 是城市入口
- `api/` 是道路系统
- `domain/` 是业务中心
- `infra/` 是水电煤网络
- `config/` 是配置中心
- `support/` 是辅助设施

---

## 3. 项目目录总览

后端核心目录在 `src/`：

```text
src/
├── api/         # HTTP 接口层
├── app_state.rs # 全局应用状态初始化
├── config/      # 配置加载与配置结构
├── domain/      # 业务逻辑核心
├── infra/       # 外部依赖封装（LLM、MCP、文件存储）
├── support/     # 日志、清洗等辅助模块
└── main.rs      # 程序入口
```

推荐先记住这一层：

- **api**：处理 HTTP
- **domain**：处理业务
- **infra**：处理外部系统
- **config**：处理配置
- **support**：处理辅助能力

---

## 4. 从 Rust 角度理解这个项目

如果你是第一次学 Rust，先把下面这些概念和本项目对应起来。

### 4.1 `struct`：数据结构

在 Rust 里，`struct` 很像“类的数据部分”。

例如：

- `AppConfig`：整个项目配置
- `AppState`：整个服务运行时状态
- `ChatRequest`：一次聊天请求的内部表示
- `ChatResult`：一次聊天结果

你可以把它理解成“后端里被频繁传递的数据盒子”。

### 4.2 `enum`：状态分支

例如：

- `ChatMode` 只有两种：`Stateless` 和 `Memory`
- `ApiError` 有 `BadRequest / NotFound / ServiceUnavailable / Internal`

`enum` 在 Rust 里非常重要，它让分支更清晰，也更安全。

### 4.3 `Result<T, E>`：可能成功，也可能失败

Rust 不鼓励随便抛异常，而是大量使用：

- `Ok(value)`：成功
- `Err(error)`：失败

本项目里很多函数返回：

```rust
Result<T>
```

意思是：这个函数可能失败，所以调用方必须处理错误。

### 4.4 `Option<T>`：这个值可能存在，也可能不存在

例如：

- `session_id: Option<String>`
- `user_id: Option<String>`

对应含义就是：有些请求会传这些字段，有些不会。

### 4.5 `async/await`：异步编程

本项目会做很多 IO：

- 网络请求 LLM
- 网络请求 MCP
- 读写文件
- 返回流式响应

这些都适合异步处理，所以你会看到很多：

```rust
async fn ...
.await
```

### 4.6 `Arc`：共享所有权

`AppState` 会被很多请求共享使用，因此项目里会用：

```rust
Arc<AppState>
```

你可以先粗略理解成：

> 这是 Rust 提供的线程安全“共享引用计数指针”，方便多个地方一起读同一份状态。

---

## 5. 后端是怎么启动起来的？

先看入口文件：`src/main.rs`。

它做了 5 件大事：

1. 找到仓库根目录
2. 加载 `.env`
3. 加载 `config/config.yaml`
4. 初始化日志
5. 构造 `AppState`，启动 Axum 服务

核心代码逻辑可以概括成：

```rust
let repo_root = find_repo_root()?;
let config = load_config(&repo_root)?;
let state = AppState::new(repo_root.clone(), config.clone())?;
axum::serve(listener, build_router(state)).await?;
```

### 5.1 你应该重点看什么？

- `find_repo_root()`：怎么找到项目根目录
- `load_repo_dotenv()`：怎么加载 `.env`
- `load_config()`：怎么读取 yaml
- `AppState::new()`：怎么初始化全局服务对象
- `build_router()`：怎么注册所有 HTTP 接口

### 5.2 为什么入口要这么简单？

好的后端入口通常应该很薄。

`main.rs` 最好只做：

- 启动
- 装配
- 不做复杂业务

本项目符合这个思路。

---

## 6. 配置系统怎么读？

相关文件：

- `src/config/model.rs`
- `src/config/loader.rs`
- `config/config.yaml`

### 6.1 `model.rs` 做什么？

它定义配置结构体，比如：

- `AppConfig`
- `AgentConfig`
- `ServerConfig`
- `McpConfig`
- `MemoryConfig`
- `SkillsConfig`

也就是“配置文件读进来以后，在 Rust 里长什么样”。

### 6.2 `loader.rs` 做什么？

它负责：

- 找到仓库根目录
- 读取 `config/config.yaml`
- 应用环境变量覆盖

也就是说：

> `model.rs` 定义形状，`loader.rs` 负责真正加载。

### 6.3 学这个模块时，重点理解什么？

重点理解三个问题：

1. YAML 如何映射成 Rust 结构体
2. 默认值是怎么给的
3. 环境变量如何覆盖配置文件

### 6.4 本项目当前关键配置有哪些？

从 `config/config.yaml` 看，最关键的是：

- `agent`：模型地址、模型名、max tokens、迭代轮数
- `server`：监听地址、端口、CORS
- `mcp`：MCP 工具服务地址
- `logging`：日志目录
- `memory`：用户记忆文件位置与上限
- `skills`：skill 学习开关和 prompt

---

## 7. 路由系统怎么读？

相关文件：

- `src/api/mod.rs`
- `src/api/routes/mod.rs`
- `src/api/routes/*.rs`

### 7.1 顶层路由入口

`src/api/mod.rs` 的 `build_router(state)` 是整个 HTTP 路由装配中心。

它主要做了这些事：

- 注册 `/health`
- 注册 `/agent/...`
- 托管 `/static`
- 托管前端首页与 fallback
- 加 TraceLayer 和 CORS

### 7.2 `/agent` 子路由

在 `src/api/routes/mod.rs` 里，把各类 agent 路由合并起来：

- `chat`
- `files`
- `memory`
- `skills`
- `tasks`
- `worktrees`
- `events`

这说明本项目的设计思路是：

> 按业务能力拆分路由文件，而不是把所有接口都塞在一个大文件里。

### 7.3 建议学生怎么读路由？

按这个顺序读：

1. `health.rs`：最简单
2. `memory.rs`：简单 JSON 读写
3. `chat.rs`：最核心
4. `skills.rs`：中等复杂度 CRUD
5. `tasks.rs` / `worktrees.rs`：理解扩展能力

---

## 8. 全局状态 `AppState` 是整个后端的核心装配点

文件：`src/app_state.rs`

### 8.1 `AppState` 里面有什么？

它把项目里最重要的服务对象装在一起，例如：

- `session_service`
- `memory_service`
- `skill_service`
- `event_service`
- `task_service`
- `worktree_service`
- `chat_orchestrator`

### 8.2 为什么需要 `AppState`？

因为 HTTP 请求处理函数不能每次都重新创建这些对象。

所以更合理的做法是：

- 程序启动时创建一次
- 所有请求共享使用

### 8.3 这里体现了什么工程思想？

这体现了一个很典型的后端工程思想：

> 把“系统依赖”集中初始化，再注入给路由和业务层。

这也是很多 Web 框架里的常见模式。

---

## 9. 一次聊天请求是怎么跑通的？

这是你学习这个项目最重要的一节。

我们以：

```text
POST /agent/run
```

为例。

### 9.1 第一步：HTTP 请求进入路由

在 `src/api/routes/chat.rs` 里：

- `agent_run()` 处理非流式无痕请求
- `agent_memory_run()` 处理非流式记忆请求
- `agent_stream()` 处理流式无痕请求
- `agent_memory_stream()` 处理流式记忆请求

### 9.2 第二步：解析 multipart/form-data

`parse_chat_multipart()` 负责把 HTTP 表单解析成内部的 `ChatRequest`：

- `message`
- `history`
- `system`
- `session_id`
- `user_id`
- `files`
- `model_id`
- `temperature`
- `max_tokens`
- `max_iterations`
- `top_p`
- MCP 覆盖项

这一层本质上是：

> 从“HTTP 输入”转成“业务层输入”。

### 9.3 第三步：路由调用编排器

解析完后，路由会调用：

```rust
state.chat_orchestrator.run(request, ChatMode::Stateless).await?
```

或者：

```rust
state.chat_orchestrator.stream(request, ChatMode::Memory, tx).await?
```

所以可以认为：

- `chat.rs` 负责“接请求”
- `orchestrator.rs` 负责“真正干活”

### 9.4 第四步：编排器组织消息、工具和循环

`src/domain/chat/orchestrator.rs` 是本项目最核心的文件。

它主要做这些事：

1. 准备请求上下文
2. 组装 system prompt
3. 加载用户记忆（如果是 memory 模式）
4. 注入 session 消息
5. 加载工具列表
6. 调用 LLM
7. 如果 LLM 要调工具，则执行工具
8. 把工具结果再塞回消息里继续下一轮
9. 得到最终回答后，提取输出文件
10. 如果是 memory 模式，再做记忆维护与 skill 学习

### 9.5 第五步：返回结果

- 非流式：返回 JSON
- 流式：返回 SSE 事件流

到这里，一次请求就完整跑完了。

---

## 10. `ChatOrchestrator` 为什么这么重要？

你可以把 `ChatOrchestrator` 理解成：

> Agent 后端的大脑调度器。

### 10.1 它解决的核心问题是什么？

大模型调用不是简单的“一问一答”，而是一个循环：

- 模型思考
- 模型发起工具调用
- 后端执行工具
- 把结果回给模型
- 模型继续思考
- 直到给出最终回答

这就是 Agent 系统和普通 Chat Completion 的本质区别。

### 10.2 `run()` 与 `stream()` 的区别

- `run()`：一次性算完，再把完整结果返回
- `stream()`：边生成边通过事件推送给前端

两者共享大部分业务逻辑，但返回方式不同。

### 10.3 学这个文件时不要一口气啃完

建议分 4 轮看：

1. 只看 `run()` 主流程
2. 再看 `stream()` 主流程
3. 再看工具调用相关函数
4. 最后看 memory / steering / compaction 这些增强能力

---

## 11. 项目里的消息结构是怎么设计的？

相关文件：`src/infra/llm/types.rs`

这是一个非常值得学习的文件，因为它把“OpenAI 兼容 chat completion 协议”映射成了 Rust 数据结构。

### 11.1 你应该认识这些结构体

- `ChatMessage`
- `ToolCall`
- `ChatCompletionRequest`
- `ChatCompletionResponse`
- `StreamChunk`
- `ToolCallAccumulator`

### 11.2 这里在训练你什么能力？

训练你把“外部 JSON 协议”转成“内部 Rust 类型系统”的能力。

这是后端工程师的基本功。

---

## 12. LLM 客户端怎么封装？

文件：`src/infra/llm/client.rs`

### 12.1 它只做两件事

- `chat()`：非流式调用模型
- `stream_chat()`：流式调用模型

### 12.2 为什么这叫 `infra`？

因为它属于“外部依赖封装”，不是业务规则本身。

换句话说：

- `domain` 关心“什么时候调用模型”
- `infra` 关心“怎么发 HTTP 请求给模型”

这就是分层的意义。

---

## 13. MCP 客户端怎么理解？

文件：`src/infra/mcp/client.rs`

如果学生第一次接触 MCP，可以先粗略理解为：

> MCP 是一种外部工具协议，这个模块负责发现工具、列出工具、调用工具。

本项目中它主要负责：

- 读取 MCP 端点配置
- 拉取工具 schema
- 在需要时调用 `mcp_*` 工具
- 支持 eager / lazy / disabled 三种暴露模式

建议第一次学习时：

- 先知道它存在
- 知道它负责“对接外部工具服务”
- 不需要一开始就把它所有细节看透

---

## 14. Session 系统是什么？

文件：`src/domain/session/service.rs`

很多初学者会问：

> 不是已经有 `history` 了吗？为什么还要 `session`？

答案是：`history` 只是聊天内容，而 `session` 还要管理运行态。

例如：

- steering 消息
- todo 状态
- background 任务
- inbox / message bus
- teammate 管理
- prompt memory snapshot cache

也就是说，`session` 保存的是：

> 一次持续协作过程中，Agent 的工作状态。

这比单纯的历史消息更丰富。

---

## 15. Memory 系统是什么？

相关文件：

- `src/domain/memory/service.rs`
- `src/infra/fs/user_memory_store.rs`

### 15.1 它解决什么问题？

让 Agent 不只记住“这一轮对话”，还记住“这个用户的长期偏好”和“这个项目的长期经验”。

### 15.2 本项目用什么做记忆？

不是数据库，而是文件：

- `USER.md`
- `MEMORY.md`

默认存放在：

```text
.user_memories/<user_id>/
```

### 15.3 为什么这种设计适合教学？

因为它非常直观：

- 学生能直接打开文件看内容
- 不需要先学数据库
- 更容易理解“记忆是如何被维护的”

### 15.4 这一套值得学生学什么？

- 如何把长期状态落盘
- 如何按用户隔离数据
- 如何在请求开始前加载记忆
- 如何在请求结束后异步/同步更新记忆

---

## 16. Skill 系统是什么？

相关文件：

- `src/domain/skills/service.rs`
- `src/infra/fs/skill_store.rs`

### 16.1 skill 可以粗略理解成什么？

可以理解成：

> 一段可复用的任务说明或操作知识，以 markdown 文件形式存在。

### 16.2 本项目里的 skill 分为哪些？

- shared：公共 skill
- private：用户私有 skill
- effective：对当前用户生效的 skill 合集

### 16.3 它是怎么存储的？

- 公共 skill 在项目 `skills/` 目录
- 用户私有 skill 在用户记忆目录下

### 16.4 学生应该从这里学到什么？

- 如何设计文件型 CRUD
- 如何做“共享 + 私有覆盖”机制
- 如何在实际运行后让系统自动学习和更新技能

---

## 17. Task / Worktree / Event 是什么？

这三个模块让系统不只是一个聊天机器人，而更像一个可执行的工程助手。

### 17.1 Task

文件：`src/domain/tasks/service.rs`

负责：

- 创建任务
- 获取任务
- 更新任务
- 任务 owner / 依赖关系管理

学生可以把它理解成一个轻量任务系统。

### 17.2 Worktree

文件：`src/domain/worktree/service.rs`

负责：

- 创建 Git worktree
- 记录 worktree 状态
- 绑定任务
- 在 worktree 里跑命令

学生可以把它理解成：

> 为多任务开发提供隔离工作目录。

### 17.3 Event

负责记录系统运行事件，便于审计和调试。

这套设计说明本项目不仅在做“聊天”，也在做“工程化执行”。

---

## 18. 前端为什么也会出现在后端里？

文件：`src/api/routes/frontend.rs`

虽然前端代码在 `frontend/`，但最终构建产物会被后端托管。

后端逻辑是：

- 如果访问的是 API，就走 API
- 如果访问的是前端路由，就返回 `static/frontend/index.html`

这是一种常见的“单服务托管前后端”的做法。

---

## 19. 日志系统怎么理解？

文件：`src/support/logging.rs`

它做了两件很实用的事：

- 日志同时输出到 stdout
- 按日期写入 `logs/backend-YYYY-MM-DD.log`

这是很适合教学的例子，因为它展示了：

- Rust 如何封装自定义 writer
- Web 服务如何做基础日志轮转

---

## 20. 推荐源码阅读顺序（非常重要）

如果你的学生完全从 0 开始，建议按这个顺序读：

### 第一阶段：先建立全局印象

1. `README.md`
2. `src/main.rs`
3. `src/app_state.rs`
4. `src/api/mod.rs`
5. `src/api/routes/mod.rs`

目标：知道系统怎么启动、路由怎么挂载。

### 第二阶段：理解聊天主链路

6. `src/api/routes/chat.rs`
7. `src/domain/chat/models.rs`
8. `src/infra/llm/types.rs`
9. `src/domain/chat/orchestrator.rs`
10. `src/infra/llm/client.rs`

目标：知道一次聊天请求怎么从 HTTP 走到 LLM。

### 第三阶段：理解核心增强能力

11. `src/domain/session/service.rs`
12. `src/domain/memory/service.rs`
13. `src/infra/fs/user_memory_store.rs`
14. `src/domain/skills/service.rs`
15. `src/infra/fs/skill_store.rs`

目标：知道 session、memory、skills 是怎么做的。

### 第四阶段：理解工程化能力

16. `src/domain/tasks/service.rs`
17. `src/domain/worktree/service.rs`
18. `src/infra/mcp/client.rs`
19. `src/support/logging.rs`

目标：知道系统如何从“聊天”变成“工程助手”。

---

## 21. 学生需要先掌握哪些 Rust 基础？

建议先掌握下面这些，再进项目会轻松很多。

### 21.1 必须掌握

- 变量、函数、模块
- `struct` / `enum`
- `impl`
- `match`
- `Vec` / `HashMap`
- `String` / `&str`
- `Option` / `Result`
- `?` 运算符
- 所有权、借用、引用
- `async/await`

### 21.2 进阶时再掌握

- trait
- 泛型
- 生命周期
- `Arc` / `Mutex`
- 错误处理库（`anyhow` / `thiserror`）
- serde 序列化与反序列化

---

## 22. 结合本项目学习 Rust：最好的方式不是背语法，而是对照源码

比如这样对照：

- 学 `struct` → 看 `AppConfig`、`ChatRequest`
- 学 `enum` → 看 `ChatMode`、`ApiError`
- 学 `Result` → 看 `load_config()`、`chat()`
- 学 `async` → 看 `agent_run()`、`stream_chat()`
- 学 `Arc` → 看 `SharedState`
- 学 serde → 看 `ChatCompletionRequest`

这样学会比单独刷语法更快。

---

## 23. 给学生的第一周学习计划

### Day 1

- 安装 Rust
- 能跑通 `cargo run`
- 能访问 `/health`
- 阅读 `README.md`
- 阅读 `src/main.rs`

### Day 2

- 阅读 `src/api/mod.rs`
- 阅读 `src/api/routes/mod.rs`
- 阅读 `src/api/routes/health.rs`
- 阅读 `src/api/routes/chat.rs` 前半部分

### Day 3

- 阅读 `src/domain/chat/models.rs`
- 阅读 `src/infra/llm/types.rs`
- 阅读 `src/infra/llm/client.rs`

### Day 4

- 阅读 `src/domain/chat/orchestrator.rs` 的 `run()` 主流程
- 画出一次请求调用链路图

### Day 5

- 阅读 `session / memory / skills` 三个模块
- 打开 `.user_memories/` 看真实文件结构

### Day 6

- 阅读 `tasks / worktree / events`
- 尝试调用对应接口

### Day 7

- 自己完成一个小改动
- 自己写一份“请求链路总结”

---

## 24. 最适合学生做的 10 个练习

### 练习 1：新增一个最简单接口

目标：新增 `GET /agent/ping`，返回：

```json
{ "message": "pong" }
```

学习点：路由、返回 JSON。

### 练习 2：给 `/health` 增加一个字段

比如增加：

- 当前端口
- 当前日志目录

学习点：读取 `state.config`。

### 练习 3：在 `chat.rs` 增加一个参数校验

比如限制某个字段不能过长。

学习点：表单解析、错误返回。

### 练习 4：给 `README` 中的某个接口写测试用例

学习点：接口测试。

### 练习 5：给 `TaskService` 增加一个简单查询方法

例如：列出 `pending` 任务。

学习点：业务服务层扩展。

### 练习 6：给 `MemoryResponse` 增加一个统计字段

例如字符数或文件路径。

学习点：DTO 设计。

### 练习 7：在日志里增加一条业务日志

学习点：`tracing`。

### 练习 8：修改一个默认配置项

学习点：配置模型 + 配置加载。

### 练习 9：读懂并解释 `run()` 和 `stream()` 的区别

学习点：非流式 / 流式业务结构。

### 练习 10：画出后端分层图

学习点：工程理解能力。

---

## 25. 学生最容易卡住的地方

### 25.1 卡在 Rust 语法

建议：不要试图先学完所有 Rust 再看项目。

更好的方式是：

- 先看项目
- 遇到语法就查
- 查完马上回源码验证

### 25.2 卡在 `async`

建议先把它理解成：

> 这是“会等待 IO 的函数”，先不用过度钻底层实现。

### 25.3 卡在所有权

建议初期先记住：

- `String` 是拥有数据的
- `&str` 是借用文本的
- `clone()` 可以先救急，但不要滥用

### 25.4 卡在大文件 `orchestrator.rs`

建议分块读，不要整文件硬啃。

---

## 26. 给老师的教学建议

如果你是带学生学习这套项目，推荐这样安排：

### 第一步：先让学生跑起来

不要先讲抽象概念，先让他们：

- `cargo run`
- 打开 `/health`
- 调一次 `/agent/run`

### 第二步：让学生画图

让学生画：

- 启动流程图
- 请求链路图
- 模块分层图

只要图画出来，理解就已经过半。

### 第三步：让学生做小修改

不要一上来布置复杂需求。

先让他们：

- 改一个字段
- 加一个简单接口
- 改一个错误提示

### 第四步：再进入复杂模块

等基础熟悉后，再让他们看：

- memory
- skills
- worktree
- mcp

---

## 27. 这个项目最值得学习的工程思想

最后，学生不只是要学“Rust 语法”，更要学这几个工程思想：

### 27.1 分层

- API 层不做重业务
- 业务逻辑集中在 domain
- 外部依赖放到 infra

### 27.2 配置和代码分离

- 配置写在 yaml
- 环境变量可以覆盖

### 27.3 用类型表达约束

- `Option` 表示可选
- `Result` 表示可能失败
- `enum` 表示有限状态

### 27.4 让主流程清晰

- 路由负责接入
- 编排器负责主链路
- 存储和外部依赖分别封装

### 27.5 让系统具备扩展性

现在这个项目已经不只是“聊天接口”，它还具备：

- 记忆
- skills
- tasks
- worktree
- MCP 工具集成
- session 状态管理

这说明它的后端结构是为扩展准备的。

---

## 28. 最后的建议：怎样才算“真正学会了这个项目”？

如果学生能完成下面 4 件事，就说明已经入门成功：

1. 能从 `main.rs` 讲清楚程序怎么启动
2. 能从 `chat.rs` 讲清楚一次请求怎么流到 `ChatOrchestrator`
3. 能解释 `memory` 和 `session` 分别解决什么问题
4. 能自己独立完成一个小功能修改并通过测试

这时，学生学到的就不只是 Rust，而是真正的**Rust 后端工程实践**。

---

## 29. 附录：建议边学边看的文件清单

### 入门清单

- `src/main.rs`
- `src/app_state.rs`
- `src/api/mod.rs`
- `src/api/routes/health.rs`
- `src/api/routes/chat.rs`

### 核心清单

- `src/domain/chat/orchestrator.rs`
- `src/domain/chat/models.rs`
- `src/infra/llm/client.rs`
- `src/infra/llm/types.rs`

### 进阶清单

- `src/domain/session/service.rs`
- `src/domain/memory/service.rs`
- `src/domain/skills/service.rs`
- `src/domain/tasks/service.rs`
- `src/domain/worktree/service.rs`
- `src/infra/mcp/client.rs`

### 配置与运行清单

- `src/config/model.rs`
- `src/config/loader.rs`
- `config/config.yaml`
- `src/support/logging.rs`

