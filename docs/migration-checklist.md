# Rust/React Migration Checklist

This checklist tracks the remaining feature gaps between this Rust + React rewrite and the reference project at `~/PycharmProjects/auto_claude_code`.

## Current Baseline

- The current `src/` matches the reference repository's `backend-rs/src/`.
- The current `frontend/src/` matches the reference repository's frontend.
- The remaining gaps are mainly in the Rust backend orchestration layer, not in the React UI.
- The remaining gaps are now mostly around deeper production-hardening and polish rather than major missing backend features.
- Already migrated: basic chat, streaming chat, memory chat, workspace file tools (`read_file` / `write_file` / `edit_file`), MCP tool loading/calling, task/worktree/event basics, skills CRUD, file download, health check, static frontend hosting.

## P0: Base Agent Tools

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | `read_file` | `app/agent/tools.py::run_read` | `src/infra/fs/tool_ops.rs` |
| DONE | `write_file` | `app/agent/tools.py::run_write` | `src/infra/fs/tool_ops.rs` |
| DONE | `edit_file` | `app/agent/tools.py::run_edit` | `src/infra/fs/tool_ops.rs` |
| DONE | Tool schema aggregation | `app/api/routes.py::get_all_tools` | `src/domain/chat/orchestrator.rs` |
| DONE | Tool dispatch | `app/api/routes.py::dispatch_tool` | `src/domain/chat/orchestrator.rs` |

Acceptance criteria:

- Uploaded text files can be read through `read_file`.
- The agent can safely write and edit files inside the workspace.
- Any path escaping the workspace is rejected.
- Unknown tools return a controlled error instead of crashing the loop.

## P1: MCP And Attachment Parsing

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | MCP `initialize` and session tracking | `app/mcp/client.py` | `src/infra/mcp/client.rs` |
| DONE | Dynamic `tools/list` loading | `app/mcp/client.py::list_mcp_tools` | `src/domain/chat/orchestrator.rs` |
| DONE | MCP `tools/call` dispatch | `app/mcp/client.py::call_mcp_tool` | `src/domain/chat/orchestrator.rs` |
| DONE | `.docx/.xlsx/.xls/.csv` parsing | `app/agent/tools.py::run_read` | `read_file` implementation + `src/infra/mcp/client.rs` |
| DONE | SSE `files_uploaded` event | `app/api/routes.py` stream handlers | `src/api/routes/chat.rs` |

Acceptance criteria:

- MCP tools are injected as `mcp_*` tools when the MCP server is available.
- MCP startup or network failure does not block normal chat.
- Office and CSV uploads can be read through MCP-backed parsing.
- Stream responses emit `files_uploaded` after successful upload handling.

## P2: Task, Worktree, And Events

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | `TaskManager` | `app/agent/core.py::TaskManager` | `src/domain/tasks` |
| DONE | `WorktreeManager` | `app/agent/core.py::WorktreeManager` | `src/domain/worktree` |
| DONE | `EventBus` | `app/agent/core.py::EventBus` | `src/domain/events` |
| DONE | `GET /agent/tasks` | `app/api/routes.py` | `src/api/routes/tasks.rs` |
| DONE | `GET /agent/worktrees` | `app/api/routes.py` | `src/api/routes/worktrees.rs` |
| DONE | `GET /agent/events` | `app/api/routes.py` | `src/api/routes/worktrees.rs` |
| DONE | Task/worktree tools | `app/agent/tools.py` | Tool schema aggregation and dispatch |

Acceptance criteria:

- HTTP responses remain compatible with the Python backend.
- The agent can create/list/update tasks.
- The agent can create/list/status/run/keep/remove worktrees.
- Worktree lifecycle events are persisted and queryable.

## P3: Session State

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | `SessionContext` | `app/agent/session.py` | `src/domain/session/service.rs` |
| DONE | `TodoWrite` | `app/agent/core.py::TodoManager` | `src/domain/session/todo.rs` |
| DONE | `BackgroundManager` | `app/agent/core.py::BackgroundManager` | `src/domain/session/background.rs` |
| DONE | `MessageBus` | `app/agent/core.py::MessageBus` | `src/domain/session/message_bus.rs` |
| DONE | `TeammateManager` | `app/agent/core.py::TeammateManager` | `src/domain/session/teammate.rs` + `src/domain/session/service.rs` |
| DONE | Prompt memory snapshot cache | `SessionContext.prompt_memory_snapshots` | session-scoped memory cache |

Acceptance criteria:

- A stable `session_id` preserves todo, background task, and inbox state.
- Different sessions do not share session-scoped state.
- Deleting a session clears its in-memory state.
- Session state has a bounded capacity or eviction policy.

## P4: Long-Running Chat Orchestration

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | `microcompact` | `app/agent/core.py::microcompact` | `src/domain/chat/compaction.rs` |
| DONE | `auto_compact` | `app/agent/core.py::auto_compact` | `src/domain/chat/compaction.rs` |
| DONE | `compress` tool | `app/agent/tools.py::COMPRESS_TOOLS` | Tool schema aggregation and dispatch |
| DONE | Background result injection | `app/api/routes.py::_agent_loop` | Chat loop pre-call injection |
| DONE | Inbox injection | `app/api/routes.py::_agent_loop` | Chat loop pre-call injection |
| DONE | Todo reminder behavior | `app/api/routes.py::_agent_loop` | Chat loop post-tool handling |

Acceptance criteria:

- Long histories do not grow without bounds.
- The agent can explicitly request compression with `compress`.
- Background task results are injected before the next model call.
- Lead inbox messages are injected before the next model call.
- Open todos trigger reminders after repeated rounds without `TodoWrite`.

## P5: API Compatibility And Tests

| Status | Feature | Reference | Rust Target |
| --- | --- | --- | --- |
| DONE | `agent_id` form field compatibility | `app/api/routes.py` memory endpoints | `ChatRequest` parsing |
| DONE | Path safety tests | `app/agent/tools.py::safe_path` | `src/infra/fs/tool_ops.rs` unit tests |
| DONE | MCP degradation tests | `app/mcp/client.py` | Rust integration/unit tests |
| DONE | Task/worktree persistence tests | `app/agent/core.py` | Rust unit tests |
| DONE | API smoke tests | All current agent endpoints | Rust/backend integration tests |
| DONE | README status update | `README.md` | document true migration state |

Acceptance criteria:

- Existing frontend requests remain compatible.
- Unsupported or not-yet-migrated features are documented clearly.
- Tests cover the highest-risk behavior: path safety, MCP failure, session isolation, and worktree state.

## Post-checklist Hardening

These items are not major Python→Rust feature gaps anymore, but are still important for making the Rust rewrite practical to run and maintain.

| Status | Improvement | Rust Target |
| --- | --- | --- |
| DONE | Env override hardening (`AGENT_*` aliases, MCP timeout overrides, blank env ignored, invalid numeric env fail-fast) | `src/config/loader.rs` |
| DONE | Runtime/deployment guide, Docker persistence mounts, config/skills bind mounts | `docs/runtime-guide.md`, `docker-compose.yml`, `README.md` |
| DONE | Frontend mode-surface alignment (UI only keeps stream + memory-stream, legacy sync paths redirect for compatibility) | `frontend/src/types.ts`, `frontend/src/utils.ts`, `frontend/src/App.tsx` |
| DONE | Frontend Vitest scaffold and baseline coverage for mode routing / scroll-button helper logic | `frontend/package.json`, `frontend/vite.config.ts`, `frontend/src/*.test.ts` |
| DONE | Local `cargo run` auto-loads repo-root `.env` while preserving shell env precedence | `src/main.rs`, `src/config/loader.rs` |

## Recommended Implementation Order

1. Implement P0 first so the agent can actually inspect and modify workspace files.
2. Implement P1 next so attachments and external MCP capabilities become usable.
3. Implement P2 to restore task/worktree/project lifecycle operations.
4. Implement P3 and P4 to restore session-based collaboration and long-running workflow behavior.
5. Finish P5 to preserve compatibility and reduce regression risk.
