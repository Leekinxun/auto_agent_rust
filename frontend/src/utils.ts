import { marked } from "marked";
import type {
  AppSettings,
  ChatModeConfig,
  ChatModeId,
  ChatState,
  DisplayMessage,
  McpExposureMode,
  McpServerPreview,
  OutputFile,
  ViewId
} from "./types";

export const SETTINGS_KEY = "auto_claude_code_frontend_settings";
export const DEFAULT_API_BASE = window.location.origin || "http://localhost:8080";
export const DEFAULT_MEMORY_USER_ID = "frontend-user";

export const CHAT_MODES: Record<ChatModeId, ChatModeConfig> = {
  stream: {
    id: "stream",
    view: "stream",
    title: "无痕流式",
    subtitle: "边生成边输出，适合长内容、工具调用和逐步观察执行过程。",
    placeholder: "测试流式输出，或要求分步骤执行任务...",
    endpoint: "/agent/stream",
    streaming: true,
    memory: false,
    sessionId: "frontend-stream",
    badges: ["流式输出", "支持附件", "过程可见"],
    suggestions: [
      "逐步分析这批文件，并实时展示处理过程",
      "边思考边输出一个调试方案",
      "用流式方式生成一份较长的项目报告"
    ]
  },
  memoryStream: {
    id: "memoryStream",
    view: "memoryStream",
    title: "记忆流式",
    subtitle: "基于 USER.md 和 MEMORY.md 的流式对话，适合连续协作。",
    placeholder: "带文件记忆的流式对话，适合连续协作...",
    endpoint: "/agent/memory/stream",
    streaming: true,
    memory: true,
    sessionId: "frontend-memory-stream",
    userId: DEFAULT_MEMORY_USER_ID,
    badges: ["文件记忆", "流式输出", "长期协作"],
    suggestions: [
      "延续之前的分析结果，继续往下做",
      "结合已经维护的文件记忆，给出新的执行建议",
      "边生成边整合已有知识，形成下一步计划"
    ]
  }
};

export const NAV_GROUPS: Array<{ title: string; items: { id: ViewId; title: string; description: string; pill: string }[] }> = [
  {
    title: "对话",
    items: [
      { id: "stream", title: "无痕流式", description: "边生成边返回", pill: "SSE" },
      { id: "memoryStream", title: "记忆流式", description: "基于文件记忆的流式协作", pill: "LIVE" }
    ]
  },
  {
    title: "管理",
    items: [
      { id: "skills", title: "Skills 管理", description: "新增、编辑、刷新", pill: "CRUD" },
      { id: "settings", title: "设置", description: "配置后端地址", pill: "API" }
    ]
  }
];

export const VIEW_PATHS: Record<ViewId, string> = {
  stream: "/chat/stream",
  memoryStream: "/chat/memory-stream",
  skills: "/skills",
  settings: "/settings"
};

export const SKILL_CREATE_PATH = "/skills/new";

export function createInitialChats(): Record<ChatModeId, ChatState> {
  return {
    stream: { history: [], messages: [], files: [], input: "", sending: false, stopRequested: false, queue: [] },
    memoryStream: { history: [], messages: [], files: [], input: "", sending: false, stopRequested: false, queue: [] }
  };
}

export function createDefaultSkillDraft(name = "") {
  const trimmedName = name.trim();
  return {
    name: trimmedName,
    description: "",
    tags: "",
    trigger: "",
    body: trimmedName
      ? `# ${trimmedName}\n\n## 作用\n\n描述这个 skill 的职责和边界。\n\n## 使用方式\n\n1. 说明触发条件\n2. 说明执行步骤\n3. 说明输出要求\n`
      : "# New Skill\n\n## 作用\n\n描述这个 skill 的职责和边界。\n\n## 使用方式\n\n1. 说明触发条件\n2. 说明执行步骤\n3. 说明输出要求\n",
    folder: suggestFolder(trimmedName || "new-skill")
  };
}

export function createId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export function getPathForView(view: ViewId) {
  return VIEW_PATHS[view];
}

export function getSkillDetailPath(name: string) {
  return `/skills/${encodeURIComponent(name)}`;
}

export function getViewFromPath(pathname: string): ViewId | null {
  const normalized = pathname.replace(/\/+$/, "") || "/";
  if (normalized === "/") {
    return "stream";
  }
  if (normalized === "/chat/normal") {
    return "stream";
  }
  if (normalized === "/chat/memory-run") {
    return "memoryStream";
  }
  if (normalized.startsWith("/skills")) {
    return "skills";
  }
  if (normalized.startsWith("/settings")) {
    return "settings";
  }
  const found = Object.entries(VIEW_PATHS).find(([, path]) => path === normalized);
  return (found?.[0] as ViewId | undefined) || null;
}

export function normalizeApiBase(value: string) {
  return (value || DEFAULT_API_BASE).trim().replace(/\/+$/, "");
}

export function normalizeMcpEndpoint(value: string) {
  return value.trim().replace(/\/+$/, "");
}

export function parseMcpBaseUrlsInput(value: string) {
  const seen = new Set<string>();
  return value
    .split(/\r?\n|,/)
    .map((item) => normalizeMcpEndpoint(item))
    .filter((item) => item.length > 0)
    .filter((item) => {
      if (seen.has(item)) {
        return false;
      }
      seen.add(item);
      return true;
    });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeMcpPreviewServers(value: unknown): McpServerPreview[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((server) => {
    if (!isRecord(server) || typeof server.endpoint !== "string") {
      return [];
    }

    const tools = Array.isArray(server.tools)
      ? server.tools.flatMap((tool) => {
        if (!isRecord(tool) || typeof tool.name !== "string") {
          return [];
        }
        return [{
          name: tool.name,
          description: typeof tool.description === "string" ? tool.description : ""
        }];
      })
      : [];

    const rawToolCount =
      typeof server.toolCount === "number"
        ? server.toolCount
        : typeof server.tool_count === "number"
          ? server.tool_count
          : tools.length;
    const rawMode = typeof server.mode === "string" ? server.mode.toLowerCase() : "";
    const mode: McpExposureMode =
      rawMode === "lazy" || rawMode === "disabled" || rawMode === "eager"
        ? rawMode
        : "eager";

    return [{
      endpoint: server.endpoint,
      endpointKey: typeof server.endpointKey === "string"
        ? server.endpointKey
        : typeof server.endpoint_key === "string"
          ? server.endpoint_key
          : server.endpoint,
      mode,
      ok: typeof server.ok === "boolean" ? server.ok : false,
      toolCount: rawToolCount,
      tools,
      error: typeof server.error === "string" ? server.error : undefined
    }];
  });
}

export function suggestFolder(value: string) {
  const cleaned = value
    .trim()
    .replace(/[\\/:*?"<>|\u0000-\u001f]+/g, "-")
    .replace(/\s+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned || "new-skill";
}

export function parseThinking(text: string) {
  const parts: Array<{ type: "thinking" | "text"; content: string }> = [];
  let cursor = 0;
  let segmentStart = 0;
  let inThinking = false;

  while (cursor < text.length) {
    if (!inThinking && text.startsWith("<think>", cursor)) {
      if (cursor > segmentStart) {
        parts.push({ type: "text", content: text.slice(segmentStart, cursor) });
      }
      inThinking = true;
      cursor += "<think>".length;
      segmentStart = cursor;
      continue;
    }

    if (inThinking && text.startsWith("</think>", cursor)) {
      parts.push({ type: "thinking", content: text.slice(segmentStart, cursor).trim() });
      inThinking = false;
      cursor += "</think>".length;
      segmentStart = cursor;
      continue;
    }

    cursor += 1;
  }

  if (!inThinking && segmentStart < text.length) {
    parts.push({ type: "text", content: text.slice(segmentStart) });
  }

  if (parts.length) {
    return parts;
  }

  return inThinking ? [] : [{ type: "text", content: text }];
}

export function stripThinkingContent(text: string) {
  return parseThinking(text)
    .filter((part) => part.type === "text")
    .map((part) => part.content)
    .join("");
}

export function renderMarkdown(markdown: string) {
  return marked.parse(markdown) as string;
}

export function collectDownloadFiles(message: DisplayMessage): OutputFile[] {
  const result = new Map<string, OutputFile>();
  message.outputFiles.forEach((file) => {
    const key = file.path || file.name;
    result.set(key, file);
  });

  return [...result.values()];
}

export function formatBytes(size: number) {
  if (size < 1024) {
    return `${size}B`;
  }
  if (size < 1024 * 1024) {
    return `${(size / 1024).toFixed(1)}KB`;
  }
  return `${(size / 1024 / 1024).toFixed(1)}MB`;
}

export function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function escapeWithLineBreaks(value: string) {
  return escapeHtml(value).replace(/\n/g, "<br>");
}

export function isChatView(view: ViewId): view is ChatModeId {
  return view === "stream" || view === "memoryStream";
}

export function getErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function appendOptionalFormData(formData: FormData, key: string, value: string) {
  const trimmed = value.trim();
  if (trimmed) {
    formData.append(key, trimmed);
  }
}

export function buildFormData(
  config: ChatModeConfig,
  history: Array<{ role: "user" | "assistant"; content: string }>,
  message: string,
  files: File[],
  settings: AppSettings
) {
  const formData = new FormData();
  formData.append("message", message);
  formData.append("history", JSON.stringify(history));
  formData.append("session_id", config.sessionId);
  if (config.memory) {
    formData.append("user_id", settings.memoryUserId.trim() || config.userId || DEFAULT_MEMORY_USER_ID);
  }
  appendOptionalFormData(formData, "mcp_config_path", settings.mcpConfigPath);
  const normalizedMcpBaseUrls = parseMcpBaseUrlsInput(settings.mcpBaseUrls);
  if (normalizedMcpBaseUrls.length) {
    formData.append("mcp_base_urls", JSON.stringify(normalizedMcpBaseUrls));
  }
  if (settings.mcpDisabledUrls.length) {
    formData.append("mcp_disabled_urls", JSON.stringify(settings.mcpDisabledUrls.map((item) => normalizeMcpEndpoint(item)).filter(Boolean)));
  }
  if (settings.mcpLazyUrls.length) {
    formData.append("mcp_lazy_urls", JSON.stringify(settings.mcpLazyUrls.map((item) => normalizeMcpEndpoint(item)).filter(Boolean)));
  }
  appendOptionalFormData(formData, "system_append", settings.agentPromptAppend);
  appendOptionalFormData(formData, "model_id", settings.modelId);
  appendOptionalFormData(formData, "temperature", settings.temperature);
  appendOptionalFormData(formData, "max_tokens", settings.maxTokens);
  appendOptionalFormData(formData, "max_iterations", settings.maxIterations);
  appendOptionalFormData(formData, "top_p", settings.topP);
  appendOptionalFormData(formData, "memory_maintenance_system", settings.memoryMaintenanceSystemPrompt);
  appendOptionalFormData(formData, "memory_maintenance_user_template", settings.memoryMaintenanceUserPrompt);
  appendOptionalFormData(formData, "skill_learning_system", settings.skillLearningSystemPrompt);
  appendOptionalFormData(formData, "skill_learning_user_template", settings.skillLearningUserPrompt);
  files.forEach((file) => formData.append("files", file));
  return formData;
}

export async function readEventStream(response: Response, onEvent: (eventName: string, payload: Record<string, unknown>) => void) {
  if (!response.body) {
    throw new Error("当前环境不支持流式读取");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }

    buffer += decoder.decode(value, { stream: true });
    const chunks = buffer.split("\n\n");
    buffer = chunks.pop() || "";

    for (const chunk of chunks) {
      const parsed = parseSseChunk(chunk);
      if (parsed) {
        onEvent(parsed.event, parsed.data);
      }
    }
  }

  if (buffer.trim()) {
    const parsed = parseSseChunk(buffer);
    if (parsed) {
      onEvent(parsed.event, parsed.data);
    }
  }
}

function parseSseChunk(chunk: string) {
  const lines = chunk.split("\n");
  let event = "";
  const dataLines: string[] = [];

  for (const line of lines) {
    if (line.startsWith("event:")) {
      event = line.slice(6).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice(5).trim());
    }
  }

  if (!event || !dataLines.length) {
    return null;
  }

  try {
    return {
      event,
      data: JSON.parse(dataLines.join("\n")) as Record<string, unknown>
    };
  } catch {
    return null;
  }
}
