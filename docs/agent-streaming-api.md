# Agent 接口文档（流式 + 非流式）

本文面向接口调用方，说明后端 `agent` 相关的 4 个主要对话接口：

- 无痕非流式：`POST /agent/run`
- 无痕流式：`POST /agent/stream`
- 记忆非流式：`POST /agent/memory/run`
- 记忆流式：`POST /agent/memory/stream`

> 说明：
>
> - 这里写的是**后端 API**，完整前缀是 `/agent/...`
> - 前端页面里的 `/chat/stream`、`/chat/memory-stream` 是页面路由，不是这些后端接口本身
> - 当前前端 UI 只直接暴露两个**流式**入口；两个**非流式**接口仍由后端保留，适合脚本、服务间调用或需要一次性拿完整 JSON 的场景

---

## 1. 接口概览

基础地址示例：

```text
http://localhost:18000
```

这 4 个接口都具备以下特征：

- 请求方式：`POST`
- 请求体：`multipart/form-data`
- 支持多轮 `history`
- 支持文件上传字段 `files`
- 支持 `session_id` 绑定会话态工具 / steering 抢占

### 1.1 差异对比

| 接口 | 返回方式 | 用途 | 长期记忆 | 是否建议传 `user_id` | 是否支持 steering |
| --- | --- | --- | --- | --- | --- |
| `POST /agent/run` | `application/json` | 单次/无痕对话 | 否 | 非必需 | 是 |
| `POST /agent/stream` | `text/event-stream` | 单次/无痕对话 | 否 | 非必需 | 是 |
| `POST /agent/memory/run` | `application/json` | 带用户记忆的持续对话 | 是 | **强烈建议** | 是 |
| `POST /agent/memory/stream` | `text/event-stream` | 带用户记忆的持续对话 | 是 | **强烈建议** | 是 |

补充说明：

- `/agent/run`、`/agent/stream` 主要依赖本次 `message + history`，不会写回长期记忆
- `/agent/memory/run`、`/agent/memory/stream` 会在请求开始前加载用户记忆快照
- `/agent/memory/run` 会在主回答返回前，同步执行记忆维护 / skill 学习；`/agent/memory/stream` 则是在主回答结束后异步执行这些动作
- 如果你希望在运行过程中做 steering，中途插入纠正消息，**必须自行传入 `session_id`**。当前 4 个接口都不会额外回传一个新的 `session_id`

---

## 2. 通用请求格式

这 4 个接口都使用 `multipart/form-data`。

### 2.1 必填 / 常用字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `message` | string | 本轮用户输入，不能为空 |
| `history` | string | JSON 数组字符串；建议显式传，未传时后端按 `[]` 处理 |

`history` 示例：

```json
[
  { "role": "user", "content": "你好" },
  { "role": "assistant", "content": "你好，请问需要什么帮助？" }
]
```

### 2.2 常用可选字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `session_id` | string | 会话 ID。用于 session 工具态、todo/background/inbox、steering 抢占 |
| `system` | string | 覆盖默认 system prompt |
| `system_append` | string | 在默认 system prompt 后追加指令 |
| `files` | file | 上传文件，可重复传多个 `files` 字段 |
| `model_id` | string | 覆盖默认模型 |
| `temperature` | number | 取值范围 `0 ~ 2` |
| `max_tokens` | integer | 必须大于 `0` |
| `max_iterations` | integer | Agent 最大迭代轮数，必须大于 `0` |
| `top_p` | number | 取值范围 `(0, 1]` |

### 2.3 记忆 / 用户相关字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `user_id` | string | 用户 ID。记忆模式下强烈建议传，用于绑定 `USER.md` / `MEMORY.md` |
| `agent_id` | string | 兼容字段；当未提供 `user_id` 时，会回退用它作为 `user_id` |

> `POST /agent/memory/run` 和 `POST /agent/memory/stream` 在不传 `user_id` 时仍可调用，但不会真正绑定到某个用户的长期记忆，通常不符合“记忆对话”的实际用途。

### 2.4 MCP / 高级覆盖字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `mcp_config_path` | string | MCP 配置文件路径 |
| `mcp_base_urls` | string | JSON 字符串数组，例如 `["http://127.0.0.1:9000"]` |
| `mcp_disabled_urls` | string | JSON 字符串数组 |
| `mcp_lazy_urls` | string | JSON 字符串数组 |
| `memory_maintenance_system` | string | 记忆维护 system prompt 覆盖 |
| `memory_maintenance_user_template` | string | 记忆维护 user prompt 模板覆盖 |
| `skill_learning_system` | string | skill 学习 system prompt 覆盖 |
| `skill_learning_user_template` | string | skill 学习 user prompt 模板覆盖 |

---

## 3. 联调速查表

### 3.1 请求参数总表

> 4 个对话接口的请求体都为 `multipart/form-data`。

| 字段 | 类型 | 必填 | 适用接口 | 说明 |
| --- | --- | --- | --- | --- |
| `message` | string | 是 | 全部 | 本轮用户输入，不能为空 |
| `history` | string | 否 | 全部 | JSON 数组字符串；建议显式传 `[]` |
| `session_id` | string | 否 | 全部 | 会话 ID，用于 session 工具态与 steering |
| `user_id` | string | 否 | `memory/run`、`memory/stream` | 用户记忆标识，记忆模式强烈建议传 |
| `agent_id` | string | 否 | `memory/run`、`memory/stream` | `user_id` 的兼容别名；仅在未传 `user_id` 时回退使用 |
| `system` | string | 否 | 全部 | 覆盖默认 system prompt |
| `system_append` | string | 否 | 全部 | 在默认 system prompt 后追加指令 |
| `files` | file | 否 | 全部 | 可重复传多个 `files` 字段 |
| `model_id` | string | 否 | 全部 | 覆盖默认模型 |
| `temperature` | number | 否 | 全部 | 取值范围 `0 ~ 2` |
| `max_tokens` | integer | 否 | 全部 | 必须大于 `0` |
| `max_iterations` | integer | 否 | 全部 | Agent 最大迭代轮数，必须大于 `0` |
| `top_p` | number | 否 | 全部 | 取值范围 `(0, 1]` |
| `mcp_config_path` | string | 否 | 全部 | MCP 配置文件路径 |
| `mcp_base_urls` | string | 否 | 全部 | 字符串数组 JSON |
| `mcp_disabled_urls` | string | 否 | 全部 | 字符串数组 JSON |
| `mcp_lazy_urls` | string | 否 | 全部 | 字符串数组 JSON |
| `memory_maintenance_system` | string | 否 | `memory/run`、`memory/stream` | 记忆维护 system prompt 覆盖 |
| `memory_maintenance_user_template` | string | 否 | `memory/run`、`memory/stream` | 记忆维护 user 模板覆盖 |
| `skill_learning_system` | string | 否 | `memory/run`、`memory/stream` | skill 学习 system prompt 覆盖 |
| `skill_learning_user_template` | string | 否 | `memory/run`、`memory/stream` | skill 学习 user 模板覆盖 |

### 3.2 成功响应总表

| 接口 | Content-Type | 成功结构 | 说明 |
| --- | --- | --- | --- |
| `POST /agent/run` | `application/json` | `{ reply, history, output_files }` | 无痕非流式 |
| `POST /agent/memory/run` | `application/json` | `{ reply, history, skills_updated, output_files }` | 记忆非流式 |
| `POST /agent/stream` | `text/event-stream` | SSE 事件流 | 无痕流式 |
| `POST /agent/memory/stream` | `text/event-stream` | SSE 事件流 | 记忆流式 |

#### 3.2.1 非流式 JSON 字段表

| 字段 | 类型 | 出现接口 | 说明 |
| --- | --- | --- | --- |
| `reply` | string | `run`、`memory/run` | Agent 最终完整回答 |
| `history` | `HistoryEntry[]` | `run`、`memory/run` | 返回给调用方的完整对话历史 |
| `skills_updated` | `SkillDocument[]` | `memory/run` | 本轮同步学习 / 更新后实际发生变化的私有 skill |
| `output_files` | `OutputFile[]` | `run`、`memory/run` | 从最终回答中识别出的输出文件列表 |

#### 3.2.2 流式 SSE 事件字段表

| 事件名 | `data` 结构 | 说明 |
| --- | --- | --- |
| `text` | `{ "text": string }` | 模型输出的可见文本片段 |
| `tool_use` | `{ "name": string, "arguments": string }` | Agent 发起工具调用 |
| `tool_result` | `{ "tool": string, "output": string }` | 工具执行结果摘要 |
| `steering` | `{ "message": string, "skipped_tools": string[] }` | 流程被 steering 抢占 |
| `files_uploaded` | `{ "files": UploadedFile[] }` | 本次上传的文件列表 |
| `output_files` | `{ "files": OutputFile[] }` | 最终回答中识别出的输出文件 |
| `done` | `{ "finish_reason": string }` | 本次流式完成 |
| `error` | `{ "detail": string }` | 本次流式出错 |

#### 3.2.3 公共结构体字段表

`HistoryEntry`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `role` | string | 如 `user` / `assistant` |
| `content` | string | 该轮文本内容 |

`OutputFile`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `name` | string | 输出文件名 |
| `path` | string | 服务端输出路径 |

`UploadedFile`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `original_name` | string | 原始文件名 |
| `saved_path` | string | 服务端保存路径 |
| `size` | integer | 文件大小（字节） |
| `content_type` | string / null | MIME 类型 |

`SkillDocument`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `name` | string | skill 名称 |
| `description` | string | skill 描述 |
| `tags` | string | 标签文本 |
| `trigger` | string | 触发提示 |
| `folder` | string | 所属目录 |
| `path` | string | skill 文件路径 |
| `body` | string | skill 正文 |
| `meta` | object | 附加元数据 |
| `scope` | string | `private` / `shared` / `effective` |

### 3.3 错误码总表

| HTTP 状态码 | 含义 | 常见触发场景 |
| --- | --- | --- |
| `400 Bad Request` | 请求参数错误 | `message` 为空、`history` 非法、数值字段越界、steering `content` 为空 |
| `404 Not Found` | 路径不存在 | URL 写错，或访问不存在的资源路径 |
| `503 Service Unavailable` | 依赖服务不可用 | 外部依赖或下游服务临时不可用 |
| `500 Internal Server Error` | 服务端内部错误 | 未归类异常、内部执行失败 |

标准错误体：

```json
{
  "detail": "错误说明"
}
```

---

## 4. 无痕非流式：`POST /agent/run`

### 4.1 适用场景

适合：

- 一次性问答
- 脚本 / 服务端调用
- 需要等任务结束后一次性拿完整 JSON
- 不需要边生成边展示

### 4.2 `curl` 示例

```bash
curl -X POST http://localhost:18000/agent/run \
  -F 'message=请分析这个项目的目录结构' \
  -F 'history=[]'
```

```bash
curl -X POST http://localhost:18000/agent/run \
  -F 'message=请先阅读 README，再总结启动方式' \
  -F 'history=[]' \
  -F 'session_id=frontend-run'
```

```bash
curl -X POST http://localhost:18000/agent/run \
  -F 'message=请读取附件并总结重点' \
  -F 'history=[]' \
  -F 'files=@./notes.txt' \
  -F 'files=@./demo.csv'
```

### 4.3 成功响应

```json
{
  "reply": "这是项目目录结构的总结。",
  "history": [
    { "role": "user", "content": "请分析这个项目的目录结构" },
    { "role": "assistant", "content": "这是项目目录结构的总结。" }
  ],
  "output_files": [
    { "name": "report.md", "path": "/app/outputs/report.md" }
  ]
}
```

### 4.4 行为说明

- 不写入用户长期记忆
- 如果传了 `session_id`，仍然会拥有 session 级能力，例如 steering、todo/background/inbox 等
- 由于是非流式接口，客户端在响应完成前拿不到中间过程事件

---

## 5. 记忆非流式：`POST /agent/memory/run`

### 5.1 适用场景

适合：

- 连续多轮协作
- 需要沉淀用户偏好 / 项目上下文
- 需要等整轮结束后，一次性拿到回答和本轮实际更新的私有 skill

### 5.2 `curl` 示例

```bash
curl -X POST http://localhost:18000/agent/memory/run \
  -F 'message=延续上次的分析，继续整理部署步骤' \
  -F 'history=[]' \
  -F 'user_id=demo-user'
```

```bash
curl -X POST http://localhost:18000/agent/memory/run \
  -F 'message=继续刚才的任务，先检查 config 目录' \
  -F 'history=[]' \
  -F 'user_id=demo-user' \
  -F 'session_id=demo-user-session'
```

```bash
curl -X POST http://localhost:18000/agent/memory/run \
  -F 'message=继续上一轮结论' \
  -F 'history=[]' \
  -F 'agent_id=demo-user'
```

### 5.3 成功响应

```json
{
  "reply": "这是延续记忆后的回答。",
  "history": [
    { "role": "user", "content": "延续上次的分析，继续整理部署步骤" },
    { "role": "assistant", "content": "这是延续记忆后的回答。" }
  ],
  "skills_updated": [
    {
      "name": "deploy-helper",
      "description": "部署步骤辅助 skill",
      "tags": "deploy,ops",
      "trigger": "部署",
      "folder": "private/demo-user",
      "path": "/app/skills/private/demo-user/deploy-helper/SKILL.md",
      "body": "# deploy-helper...",
      "meta": {},
      "scope": "private"
    }
  ],
  "output_files": []
}
```

### 5.4 行为说明

- 请求开始前会注入该用户已有记忆快照
- 响应返回前会同步执行记忆维护，并按需学习 / 更新私有 skill
- `skills_updated` 只表示本轮实际新增或改写成功的 skill；为空不代表主回答失败

---

## 6. 无痕流式：`POST /agent/stream`

### 6.1 适用场景

适合：

- 一次性问答
- 不需要写回长期记忆
- 前端临时分析 / 临时工具调用
- 需要边生成边展示

### 6.2 `curl` 示例

```bash
curl -N -X POST http://localhost:18000/agent/stream \
  -F 'message=请分析这个项目的目录结构' \
  -F 'history=[]'
```

```bash
curl -N -X POST http://localhost:18000/agent/stream \
  -F 'message=请先阅读 README，再总结启动方式' \
  -F 'history=[]' \
  -F 'session_id=frontend-stream'
```

```bash
curl -N -X POST http://localhost:18000/agent/stream \
  -F 'message=请读取附件并总结重点' \
  -F 'history=[]' \
  -F 'files=@./notes.txt' \
  -F 'files=@./demo.csv'
```

### 6.3 行为说明

- 不写入用户长期记忆
- 如果传了 `session_id`，仍然会拥有 session 级能力，例如 steering、todo/background/inbox 等
- 如果传了 `user_id`，可见 skill 范围会按该用户解析，但不会做长期记忆维护

---

## 7. 记忆流式：`POST /agent/memory/stream`

### 7.1 适用场景

适合：

- 连续多轮协作
- 需要沉淀用户偏好 / 项目上下文
- 需要边生成边展示
- 需要把历史经验写回用户记忆或私有 skill

### 7.2 `curl` 示例

```bash
curl -N -X POST http://localhost:18000/agent/memory/stream \
  -F 'message=延续上次的分析，继续整理部署步骤' \
  -F 'history=[]' \
  -F 'user_id=demo-user'
```

```bash
curl -N -X POST http://localhost:18000/agent/memory/stream \
  -F 'message=继续刚才的任务，先检查 config 目录' \
  -F 'history=[]' \
  -F 'user_id=demo-user' \
  -F 'session_id=demo-user-session'
```

```bash
curl -N -X POST http://localhost:18000/agent/memory/stream \
  -F 'message=继续上一轮结论' \
  -F 'history=[]' \
  -F 'agent_id=demo-user'
```

### 7.3 行为说明

- 请求开始前会注入该用户已有记忆快照
- 主回答结束后，异步维护用户记忆，并按需学习 / 更新私有 skill
- 当前流式接口不会额外返回“记忆维护完成”的单独事件

---

## 8. SSE 返回事件（流式接口）

### 8.1 基本格式

```text
event: text
data: {"text":"我先看一下项目结构。"}
```

### 8.2 事件类型

| 事件名 | `data` 结构 | 说明 |
| --- | --- | --- |
| `text` | `{ "text": string }` | 模型输出的可见文本片段 |
| `tool_use` | `{ "name": string, "arguments": string }` | Agent 发起工具调用 |
| `tool_result` | `{ "tool": string, "output": string }` | 工具执行结果摘要 |
| `steering` | `{ "message": string, "skipped_tools": string[] }` | 流程被 steering 抢占，剩余工具被跳过 |
| `files_uploaded` | `{ "files": UploadedFile[] }` | 本次上传的文件列表 |
| `output_files` | `{ "files": OutputFile[] }` | 最终回答中识别出的输出文件 |
| `done` | `{ "finish_reason": string }` | 本次流式完成 |
| `error` | `{ "detail": string }` | 本次流式出错 |

### 8.3 `done.finish_reason` 常见值

- `stop`：正常结束
- `max_iterations`：达到最大迭代轮数
- `empty`：模型未返回有效结果
- 也可能出现底层模型原样返回的其他结束原因

### 8.4 典型事件顺序

```text
files_uploaded? -> text* -> (tool_use -> tool_result)* -> output_files? -> done
```

异常情况下可能是：

```text
files_uploaded? -> text* -> error
```

### 8.5 关于 `skills_updated`

协议层保留了 `skills_updated` 事件结构，但当前版本的流式主链路通常**不会实际下发**这个事件；如果你只对接现有后端，实现时可以先按上表中的实际事件处理。

---

## 9. 调用建议

### 9.1 非流式接口：直接按普通 JSON 请求处理

```ts
async function runAgent() {
  const form = new FormData();
  form.append("message", "请分析这个仓库");
  form.append("history", "[]");

  const resp = await fetch("/agent/run", {
    method: "POST",
    body: form,
  });

  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}`);
  }

  const data = await resp.json();
  console.log(data.reply, data.history, data.output_files);
}
```

### 9.2 流式接口：不要用 `EventSource` 直接发请求

因为流式接口是：

- `POST`
- `multipart/form-data`

浏览器端通常应使用：

- `fetch`
- `ReadableStream`
- 自己解析 SSE 帧

### 9.3 浏览器端流式最小示例

```ts
async function streamAgent() {
  const form = new FormData();
  form.append("message", "请分析这个仓库");
  form.append("history", "[]");
  form.append("session_id", "frontend-stream");

  const resp = await fetch("/agent/stream", {
    method: "POST",
    body: form,
  });

  if (!resp.ok || !resp.body) {
    throw new Error(`HTTP ${resp.status}`);
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });

    const frames = buffer.split("\n\n");
    buffer = frames.pop() ?? "";

    for (const frame of frames) {
      const eventLine = frame
        .split("\n")
        .find((line) => line.startsWith("event: "));
      const dataLine = frame
        .split("\n")
        .find((line) => line.startsWith("data: "));

      if (!eventLine || !dataLine) continue;

      const event = eventLine.slice("event: ".length);
      const data = JSON.parse(dataLine.slice("data: ".length));

      console.log(event, data);
    }
  }
}
```

---

## 10. 参数校验与错误

参数校验与错误码的联调速查，优先参考：

- [3.1 请求参数总表](#31-请求参数总表)
- [3.3 错误码总表](#33-错误码总表)

补充几条最常见的服务端校验结论：

- `message` 为空：`400 Bad Request`
- `history` 不是合法 JSON / 顶层不是数组：`400 Bad Request`
- `temperature` 不在 `0 ~ 2`：`400 Bad Request`
- `top_p` 不在 `(0, 1]`：`400 Bad Request`
- `max_tokens <= 0`：`400 Bad Request`
- `max_iterations <= 0`：`400 Bad Request`
- `mcp_base_urls` / `mcp_disabled_urls` / `mcp_lazy_urls` 不是字符串数组 JSON：`400 Bad Request`

典型错误体：

```json
{
  "detail": "message 不能为空"
}
```

---

## 11. 附：steering 抢占接口

如果你要在 Agent 运行中途追加纠正消息，可调用：

```text
POST /agent/session/{session_id}/steering
```

### 11.1 前提

- 发起请求时已经传了 `session_id`
- 该 `session_id` 下当前确实有一个正在运行的 Agent

否则会返回类似错误：

```json
{
  "detail": "当前没有正在运行的 agent，无法注入 steering"
}
```

### 11.2 请求格式

支持两种请求体：

1. `application/x-www-form-urlencoded`
2. `application/json`

字段都只有一个：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `content` | string | 追加给正在运行 Agent 的 steering 消息，不能为空 |

### 11.3 示例

```bash
curl -X POST http://localhost:18000/agent/session/frontend-stream/steering \
  -d 'content=停止继续读文件，先总结当前结论'
```

```bash
curl -X POST http://localhost:18000/agent/session/frontend-stream/steering \
  -H 'Content-Type: application/json' \
  -d '{"content":"不要继续调用工具，先回答我刚刚的问题"}'
```

成功响应：

```json
{
  "status": "queued",
  "session_id": "frontend-stream"
}
```

### 11.4 抢占语义

当 steering 生效时，Agent 会在**当前工具执行完成后**立即检查 steering 队列：

- 如有 steering 消息，会跳过本轮剩余尚未执行的工具
- 将 steering 消息注入上下文
- 立刻开始下一轮推理

因此，steering 不是“等整轮工具全部执行完再处理”，而是“尽快在工具边界抢占”。
