import { useEffect, useRef, useState, type MutableRefObject } from "react";
import { Navigate, Route, Routes, useLocation, useNavigate, useParams } from "react-router-dom";
import type {
  AppSettings,
  ChatModeConfig,
  ChatModeId,
  ChatState,
  DisplayMessage,
  HealthState,
  HistoryEntry,
  McpExposureMode,
  OutputFile,
  ProcessItem,
  QueuedChatSubmission,
  McpServerPreview,
  SkillEditorState,
  SkillItem,
  SkillScope,
  ToastItem,
  ViewId
} from "./types";
import {
  CHAT_MODES,
  DEFAULT_MEMORY_USER_ID,
  NAV_GROUPS,
  SETTINGS_KEY,
  buildFormData,
  collectDownloadFiles,
  createDefaultSkillDraft,
  createId,
  createInitialChats,
  escapeWithLineBreaks,
  formatBytes,
  getErrorMessage,
  getPathForView,
  getSkillDetailPath,
  getViewFromPath,
  isChatView,
  normalizeApiBase,
  normalizeMcpEndpoint,
  normalizeMcpPreviewServers,
  parseMcpBaseUrlsInput,
  parseThinking,
  readEventStream,
  renderMarkdown,
  SKILL_CREATE_PATH,
  stripThinkingContent,
  suggestFolder
} from "./utils";
import {
  formatUnreadCount,
  getScrollButtonLabel,
  getViewportFollowDelta,
  hasScrollButtonUnreadAccent,
  isBottomWithinFollowThreshold,
  isNearScrollableBottom
} from "./chatScroll";

type PromptPreviewState = {
  loading: boolean;
  error: string;
  statelessPrompt: string;
  memoryPrompt: string;
  memoryMaintenanceSystem: string;
  memoryMaintenanceUserTemplate: string;
  skillLearningSystem: string;
  skillLearningUserTemplate: string;
};

type McpPreviewState = {
  loading: boolean;
  error: string;
  servers: McpServerPreview[];
};

type ChatTurnResult = {
  reply: string;
  aborted: boolean;
};

type McpHealthCache = {
  key: string;
  checkedAt: number;
  servers: McpServerPreview[];
  error: string | null;
};

const DEFAULT_SETTINGS: AppSettings = {
  apiBase: normalizeApiBase(window.location.origin || "http://localhost:8080"),
  brandTitle: "中科院智能体平台",
  brandSubtitle: "统一承载多模式智能体对话、技能管理与平台配置。",
  memoryUserId: DEFAULT_MEMORY_USER_ID,
  mcpConfigPath: "",
  mcpBaseUrls: "",
  mcpDisabledUrls: [],
  mcpLazyUrls: [],
  agentPromptAppend: "",
  modelId: "",
  temperature: "",
  maxTokens: "",
  maxIterations: "",
  topP: "",
  memoryMaintenanceSystemPrompt: "",
  memoryMaintenanceUserPrompt: "",
  skillLearningSystemPrompt: "",
  skillLearningUserPrompt: ""
};

const SKILL_EDITOR_STORAGE_KEY = "auto_claude_code_frontend_skill_editor_v1";
const SKILL_SCOPE_STORAGE_KEY = "auto_claude_code_frontend_skill_scope_v1";
const SIDEBAR_COLLAPSED_KEY = "auto_claude_code_frontend_sidebar_collapsed";
const STREAM_REVEAL_INTERVAL_MS = 16;
const HEALTH_RECHECK_INTERVAL_MS = 15000;
const MCP_HEALTH_CACHE_MS = 60000;
const AUTO_SCROLL_BOTTOM_THRESHOLD_PX = 160;
const SCROLL_TO_BOTTOM_BUTTON_THRESHOLD_PX = 320;
const STREAMING_REPLY_BOTTOM_THRESHOLD_PX = 24;
const STREAMING_VIEWPORT_FOLLOW_MARGIN_PX = 24;
const STREAMING_VIEWPORT_FOLLOW_THRESHOLD_PX = 48;

type ScrollContainer = HTMLElement | Window;

function getStreamRevealStep(queueLength: number) {
  if (queueLength > 280) {
    return 18;
  }
  if (queueLength > 160) {
    return 10;
  }
  if (queueLength > 80) {
    return 6;
  }
  if (queueLength > 24) {
    return 3;
  }
  return 1;
}

function normalizeMcpUrlList(values: string[]) {
  const seen = new Set<string>();
  return values
    .map((value) => normalizeMcpEndpoint(value))
    .filter((value) => value.length > 0)
    .filter((value) => {
      if (seen.has(value)) {
        return false;
      }
      seen.add(value);
      return true;
    });
}

function isMcpServerDisabled(endpoint: string, disabledUrls: string[]) {
  const normalized = normalizeMcpEndpoint(endpoint);
  return normalizeMcpUrlList(disabledUrls).includes(normalized);
}

function isMcpServerLazy(endpoint: string, lazyUrls: string[]) {
  const normalized = normalizeMcpEndpoint(endpoint);
  return normalizeMcpUrlList(lazyUrls).includes(normalized);
}

function getMcpServerMode(endpoint: string, disabledUrls: string[], lazyUrls: string[]): McpExposureMode {
  if (isMcpServerDisabled(endpoint, disabledUrls)) {
    return "disabled";
  }
  if (isMcpServerLazy(endpoint, lazyUrls)) {
    return "lazy";
  }
  return "eager";
}

function buildMcpCacheKey(apiBase: string, configPath: string, rawBaseUrls: string, disabledUrls: string[], lazyUrls: string[]) {
  return JSON.stringify({
    apiBase: normalizeApiBase(apiBase || DEFAULT_SETTINGS.apiBase),
    configPath: configPath.trim(),
    baseUrls: parseMcpBaseUrlsInput(rawBaseUrls),
    disabledUrls: normalizeMcpUrlList(disabledUrls).sort(),
    lazyUrls: normalizeMcpUrlList(lazyUrls).sort()
  });
}

function summarizeMcpServers(servers: McpServerPreview[]) {
  return {
    okCount: servers.filter((server) => server.ok).length,
    totalServers: servers.length,
    totalTools: servers.reduce((sum, server) => sum + (typeof server.toolCount === "number" ? server.toolCount : server.tools.length), 0)
  };
}

async function copyTextToClipboard(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  document.body.removeChild(textarea);
}

function isScrollableOverflow(value: string) {
  return value === "auto" || value === "scroll" || value === "overlay";
}

function isWindowScrollContainer(container: ScrollContainer): container is Window {
  return "scrollY" in container;
}

function resolveScrollContainer(anchor: HTMLElement | null): ScrollContainer | null {
  let current = anchor?.parentElement ?? null;

  while (current) {
    if (isScrollableOverflow(window.getComputedStyle(current).overflowY)) {
      return current;
    }
    current = current.parentElement;
  }

  return typeof window !== "undefined" ? window : null;
}

function getDistanceToBottom(container: ScrollContainer) {
  if (isWindowScrollContainer(container)) {
    const doc = document.documentElement;
    const scrollTop = window.scrollY || doc.scrollTop || 0;
    return Math.max(0, doc.scrollHeight - (scrollTop + window.innerHeight));
  }

  return Math.max(0, container.scrollHeight - (container.scrollTop + container.clientHeight));
}

function isNearBottom(container: ScrollContainer, threshold = AUTO_SCROLL_BOTTOM_THRESHOLD_PX) {
  return getDistanceToBottom(container) <= threshold;
}

function scrollContainerToBottom(container: ScrollContainer, behavior: ScrollBehavior) {
  if (isWindowScrollContainer(container)) {
    const doc = document.documentElement;
    const body = document.body;
    const top = Math.max(
      doc.scrollHeight,
      body?.scrollHeight ?? 0,
      doc.offsetHeight,
      body?.offsetHeight ?? 0
    );
    window.scrollTo({ top, behavior });
    return;
  }

  container.scrollTo({
    top: container.scrollHeight,
    behavior
  });
}

function getScrollContainerViewportBounds(container: ScrollContainer) {
  if (isWindowScrollContainer(container)) {
    return {
      top: 0,
      bottom: window.innerHeight
    };
  }

  const rect = container.getBoundingClientRect();
  return {
    top: rect.top,
    bottom: rect.bottom
  };
}

function scrollContainerBy(container: ScrollContainer, top: number, behavior: ScrollBehavior) {
  if (top <= 0) {
    return;
  }

  if (isWindowScrollContainer(container)) {
    window.scrollBy({ top, behavior });
    return;
  }

  container.scrollBy({ top, behavior });
}

function createEditorState(skill?: SkillItem): SkillEditorState {
  if (skill) {
    return {
      mode: "edit",
      originalName: skill.name,
      folderTouched: true,
      draft: {
        name: skill.name,
        description: skill.description,
        tags: skill.tags,
        trigger: skill.trigger,
        body: skill.body,
        folder: skill.folder
      }
    };
  }

  return {
    mode: "create",
    originalName: null,
    folderTouched: false,
    draft: createDefaultSkillDraft()
  };
}

function buildDownloadUrl(apiBase: string, file: OutputFile) {
  const rawPath = String(file.path || file.name || "").trim();
  const normalized = rawPath.replace(/\\/g, "/");
  const knownRoot = ["/outputs/", "/uploads/"].find((segment) => normalized.includes(segment));
  const routePath = knownRoot
    ? normalized.slice(normalized.lastIndexOf(knownRoot) + 1)
    : normalized.startsWith("/")
      ? normalized.replace(/^\/+/, "")
      : normalized;
  return `${apiBase}/agent/download/${encodeURI(routePath)}`;
}

function getViewHeading(view: ViewId) {
  if (isChatView(view)) {
    const config = CHAT_MODES[view];
    return {
      eyebrow: config.streaming ? "Streaming Workspace" : "Chat Workspace",
      title: config.title,
      description: config.subtitle
    };
  }

  if (view === "skills") {
    return {
      eyebrow: "Skills Studio",
      title: "Skills 管理",
      description: "支持切换公用技能与当前用户的私有技能，私有技能会在记忆模式下持续沉淀经验。"
    };
  }

  return {
    eyebrow: "Settings",
    title: "设置",
    description: "配置后端地址、品牌区文案和 LLM 请求参数。设置保存在当前浏览器的本地存储中。"
  };
}

function normalizeOptionalNumericSetting(
  value: string,
  label: string,
  mode: "int" | "float",
  min?: number,
  max?: number
) {
  const trimmed = value.trim();
  if (!trimmed) {
    return "";
  }

  const parsed = mode === "int" ? Number.parseInt(trimmed, 10) : Number(trimmed);
  const isValidInteger = mode === "int" ? /^-?\d+$/.test(trimmed) : true;
  if (!Number.isFinite(parsed) || !isValidInteger) {
    throw new Error(`${label} 格式不正确`);
  }
  if (min !== undefined && parsed < min) {
    throw new Error(`${label} 不能小于 ${min}`);
  }
  if (max !== undefined && parsed > max) {
    throw new Error(`${label} 不能大于 ${max}`);
  }

  return mode === "int" ? String(parsed) : String(parsed);
}

function loadSidebarCollapsed() {
  try {
    return window.localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

function loadSkillScope(): SkillScope {
  try {
    const raw = window.localStorage.getItem(SKILL_SCOPE_STORAGE_KEY);
    return raw === "private" ? "private" : "shared";
  } catch {
    return "shared";
  }
}

function getNavIcon(view: ViewId) {
  switch (view) {
    case "stream":
      return "流";
    case "memoryStream":
      return "续";
    case "skills":
      return "技";
    case "settings":
      return "设";
    default:
      return "•";
  }
}

function loadSettings(): AppSettings {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY);
    if (!raw) {
      return DEFAULT_SETTINGS;
    }
    const parsed = JSON.parse(raw) as Partial<AppSettings>;
    return {
      apiBase: normalizeApiBase(parsed.apiBase || DEFAULT_SETTINGS.apiBase),
      brandTitle: typeof parsed.brandTitle === "string" && parsed.brandTitle.trim()
        ? parsed.brandTitle
        : DEFAULT_SETTINGS.brandTitle,
      brandSubtitle: typeof parsed.brandSubtitle === "string" && parsed.brandSubtitle.trim()
        ? parsed.brandSubtitle
        : DEFAULT_SETTINGS.brandSubtitle,
      memoryUserId: typeof parsed.memoryUserId === "string" && parsed.memoryUserId.trim()
        ? parsed.memoryUserId.trim()
        : DEFAULT_SETTINGS.memoryUserId,
      mcpConfigPath: typeof parsed.mcpConfigPath === "string" ? parsed.mcpConfigPath : DEFAULT_SETTINGS.mcpConfigPath,
      mcpBaseUrls: typeof parsed.mcpBaseUrls === "string" ? parsed.mcpBaseUrls : DEFAULT_SETTINGS.mcpBaseUrls,
      mcpDisabledUrls: Array.isArray(parsed.mcpDisabledUrls)
        ? normalizeMcpUrlList(parsed.mcpDisabledUrls.filter((item): item is string => typeof item === "string"))
        : DEFAULT_SETTINGS.mcpDisabledUrls,
      mcpLazyUrls: Array.isArray(parsed.mcpLazyUrls)
        ? normalizeMcpUrlList(parsed.mcpLazyUrls.filter((item): item is string => typeof item === "string"))
        : DEFAULT_SETTINGS.mcpLazyUrls,
      agentPromptAppend: typeof parsed.agentPromptAppend === "string" ? parsed.agentPromptAppend : DEFAULT_SETTINGS.agentPromptAppend,
      modelId: typeof parsed.modelId === "string" ? parsed.modelId : DEFAULT_SETTINGS.modelId,
      temperature: typeof parsed.temperature === "string" ? parsed.temperature : DEFAULT_SETTINGS.temperature,
      maxTokens: typeof parsed.maxTokens === "string" ? parsed.maxTokens : DEFAULT_SETTINGS.maxTokens,
      maxIterations: typeof parsed.maxIterations === "string" ? parsed.maxIterations : DEFAULT_SETTINGS.maxIterations,
      topP: typeof parsed.topP === "string" ? parsed.topP : DEFAULT_SETTINGS.topP,
      memoryMaintenanceSystemPrompt: typeof parsed.memoryMaintenanceSystemPrompt === "string"
        ? parsed.memoryMaintenanceSystemPrompt
        : DEFAULT_SETTINGS.memoryMaintenanceSystemPrompt,
      memoryMaintenanceUserPrompt: typeof parsed.memoryMaintenanceUserPrompt === "string"
        ? parsed.memoryMaintenanceUserPrompt
        : DEFAULT_SETTINGS.memoryMaintenanceUserPrompt,
      skillLearningSystemPrompt: typeof parsed.skillLearningSystemPrompt === "string"
        ? parsed.skillLearningSystemPrompt
        : DEFAULT_SETTINGS.skillLearningSystemPrompt,
      skillLearningUserPrompt: typeof parsed.skillLearningUserPrompt === "string"
        ? parsed.skillLearningUserPrompt
        : DEFAULT_SETTINGS.skillLearningUserPrompt
    };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

function loadSkillEditor(): SkillEditorState {
  try {
    const raw = window.localStorage.getItem(SKILL_EDITOR_STORAGE_KEY);
    if (!raw) {
      return createEditorState();
    }

    const parsed = JSON.parse(raw) as Partial<SkillEditorState>;
    const draft = parsed.draft;
    if (!draft || typeof draft !== "object") {
      return createEditorState();
    }

    return {
      mode: parsed.mode === "edit" ? "edit" : "create",
      originalName: typeof parsed.originalName === "string" ? parsed.originalName : null,
      folderTouched: Boolean(parsed.folderTouched),
      draft: {
        name: typeof draft.name === "string" ? draft.name : "",
        description: typeof draft.description === "string" ? draft.description : "",
        tags: typeof draft.tags === "string" ? draft.tags : "",
        trigger: typeof draft.trigger === "string" ? draft.trigger : "",
        body: typeof draft.body === "string" ? draft.body : createDefaultSkillDraft().body,
        folder: typeof draft.folder === "string" ? draft.folder : createDefaultSkillDraft().folder
      }
    };
  } catch {
    return createEditorState();
  }
}

function buildConversationTurns(messages: DisplayMessage[]) {
  const turns: Array<{ id: string; user?: DisplayMessage; assistant?: DisplayMessage }> = [];
  let pendingUser: DisplayMessage | null = null;

  for (const message of messages) {
    if (message.role === "user") {
      if (pendingUser) {
        turns.push({ id: pendingUser.id, user: pendingUser });
      }
      pendingUser = message;
      continue;
    }

    if (pendingUser) {
      turns.push({
        id: `${pendingUser.id}-${message.id}`,
        user: pendingUser,
        assistant: message
      });
      pendingUser = null;
      continue;
    }

    turns.push({ id: message.id, assistant: message });
  }

  if (pendingUser) {
    turns.push({ id: pendingUser.id, user: pendingUser });
  }

  return turns;
}

function draftMatchesSkill(editor: SkillEditorState, skill: SkillItem | null) {
  if (!skill || editor.mode !== "edit") {
    return false;
  }

  return (
    editor.draft.name === skill.name &&
    editor.draft.description === skill.description &&
    editor.draft.tags === skill.tags &&
    editor.draft.trigger === skill.trigger &&
    editor.draft.body === skill.body &&
    editor.draft.folder === skill.folder
  );
}

function isSkillEditorDirty(editor: SkillEditorState, skill: SkillItem | null) {
  if (editor.mode === "create") {
    const base = createDefaultSkillDraft();
    return (
      editor.draft.name.trim() !== "" ||
      editor.draft.description.trim() !== "" ||
      editor.draft.tags.trim() !== "" ||
      editor.draft.trigger.trim() !== "" ||
      editor.draft.folder !== base.folder ||
      editor.draft.body.trim() !== base.body.trim()
    );
  }

  return !draftMatchesSkill(editor, skill);
}

export default function App() {
  const navigate = useNavigate();
  const location = useLocation();
  const currentView = getViewFromPath(location.pathname) || "stream";

  const [settings, setSettings] = useState<AppSettings>(loadSettings);
  const [draftApiBase, setDraftApiBase] = useState(settings.apiBase);
  const [draftBrandTitle, setDraftBrandTitle] = useState(settings.brandTitle);
  const [draftBrandSubtitle, setDraftBrandSubtitle] = useState(settings.brandSubtitle);
  const [draftMemoryUserId, setDraftMemoryUserId] = useState(settings.memoryUserId);
  const [draftMcpConfigPath, setDraftMcpConfigPath] = useState(settings.mcpConfigPath);
  const [draftMcpBaseUrls, setDraftMcpBaseUrls] = useState(settings.mcpBaseUrls);
  const [draftMcpDisabledUrls, setDraftMcpDisabledUrls] = useState<string[]>(settings.mcpDisabledUrls);
  const [draftMcpLazyUrls, setDraftMcpLazyUrls] = useState<string[]>(settings.mcpLazyUrls);
  const [draftAgentPromptAppend, setDraftAgentPromptAppend] = useState(settings.agentPromptAppend);
  const [draftModelId, setDraftModelId] = useState(settings.modelId);
  const [draftTemperature, setDraftTemperature] = useState(settings.temperature);
  const [draftMaxTokens, setDraftMaxTokens] = useState(settings.maxTokens);
  const [draftMaxIterations, setDraftMaxIterations] = useState(settings.maxIterations);
  const [draftTopP, setDraftTopP] = useState(settings.topP);
  const [draftMemoryMaintenanceSystemPrompt, setDraftMemoryMaintenanceSystemPrompt] = useState(settings.memoryMaintenanceSystemPrompt);
  const [draftMemoryMaintenanceUserPrompt, setDraftMemoryMaintenanceUserPrompt] = useState(settings.memoryMaintenanceUserPrompt);
  const [draftSkillLearningSystemPrompt, setDraftSkillLearningSystemPrompt] = useState(settings.skillLearningSystemPrompt);
  const [draftSkillLearningUserPrompt, setDraftSkillLearningUserPrompt] = useState(settings.skillLearningUserPrompt);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(loadSidebarCollapsed);
  const [health, setHealth] = useState<HealthState>({ tone: "loading", label: "连接中..." });
  const [chats, setChats] = useState<Record<ChatModeId, ChatState>>(createInitialChats);
  const chatsRef = useRef<Record<ChatModeId, ChatState>>(chats);
  const activeStreamControllersRef = useRef<Record<ChatModeId, AbortController | null>>({
    stream: null,
    memoryStream: null
  });
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [skillScope, setSkillScope] = useState<SkillScope>(loadSkillScope);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [skillsSaving, setSkillsSaving] = useState(false);
  const [skillsDeleting, setSkillsDeleting] = useState(false);
  const [skillEditor, setSkillEditor] = useState<SkillEditorState>(loadSkillEditor);
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [promptPreview, setPromptPreview] = useState<PromptPreviewState>({
    loading: false,
    error: "",
    statelessPrompt: "",
    memoryPrompt: "",
    memoryMaintenanceSystem: "",
    memoryMaintenanceUserTemplate: "",
    skillLearningSystem: "",
    skillLearningUserTemplate: ""
  });
  const [mcpPreview, setMcpPreview] = useState<McpPreviewState>({
    loading: false,
    error: "",
    servers: []
  });
  const mcpHealthCacheRef = useRef<McpHealthCache | null>(null);

  useEffect(() => {
    window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    setDraftApiBase(settings.apiBase);
    setDraftBrandTitle(settings.brandTitle);
    setDraftBrandSubtitle(settings.brandSubtitle);
    setDraftMemoryUserId(settings.memoryUserId);
    setDraftMcpConfigPath(settings.mcpConfigPath);
    setDraftMcpBaseUrls(settings.mcpBaseUrls);
    setDraftMcpDisabledUrls(settings.mcpDisabledUrls);
    setDraftMcpLazyUrls(settings.mcpLazyUrls);
    setDraftAgentPromptAppend(settings.agentPromptAppend);
    setDraftModelId(settings.modelId);
    setDraftTemperature(settings.temperature);
    setDraftMaxTokens(settings.maxTokens);
    setDraftMaxIterations(settings.maxIterations);
    setDraftTopP(settings.topP);
    setDraftMemoryMaintenanceSystemPrompt(settings.memoryMaintenanceSystemPrompt);
    setDraftMemoryMaintenanceUserPrompt(settings.memoryMaintenanceUserPrompt);
    setDraftSkillLearningSystemPrompt(settings.skillLearningSystemPrompt);
    setDraftSkillLearningUserPrompt(settings.skillLearningUserPrompt);
  }, [settings]);

  useEffect(() => {
    window.localStorage.setItem(SKILL_EDITOR_STORAGE_KEY, JSON.stringify(skillEditor));
  }, [skillEditor]);

  useEffect(() => {
    window.localStorage.setItem(SKILL_SCOPE_STORAGE_KEY, skillScope);
  }, [skillScope]);

  useEffect(() => {
    window.localStorage.removeItem("auto_claude_code_frontend_chats_v1");
  }, []);

  useEffect(() => {
    chatsRef.current = chats;
  }, [chats]);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_COLLAPSED_KEY, sidebarCollapsed ? "1" : "0");
  }, [sidebarCollapsed]);

  useEffect(() => {
    document.title = settings.brandTitle.trim() || DEFAULT_SETTINGS.brandTitle;
  }, [settings.brandTitle]);

  useEffect(() => {
    let cancelled = false;

    async function run(silent = false) {
      if (!silent) {
        setHealth({ tone: "loading", label: "连接中..." });
      }
      try {
        const data = await testHealth(settings.apiBase);
        if (cancelled) {
          return;
        }
        setHealth({
          tone: "ok",
          label: `${String(data.model || "unknown")} · ${String(data.status || "ok")}`
        });
      } catch {
        if (cancelled) {
          return;
        }
        setHealth({ tone: "error", label: "连接失败" });
      }
    }

    void run();
    const timer = window.setInterval(() => {
      void run(true);
    }, HEALTH_RECHECK_INTERVAL_MS);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [settings.apiBase]);

  useEffect(() => {
    if (currentView !== "settings") {
      return;
    }

    let cancelled = false;
    setPromptPreview((current) => ({ ...current, loading: true, error: "" }));

    void Promise.all([
      fetchPromptPreview(draftApiBase, draftMemoryUserId),
      fetchAgentPromptSettings(draftApiBase)
    ])
      .then(([systemPromptData, agentPromptData]) => {
        if (cancelled) {
          return;
        }
        setPromptPreview({
          loading: false,
          error: "",
          statelessPrompt: typeof systemPromptData.stateless_prompt === "string" ? systemPromptData.stateless_prompt : "",
          memoryPrompt: typeof systemPromptData.memory_prompt === "string" ? systemPromptData.memory_prompt : "",
          memoryMaintenanceSystem: typeof agentPromptData.memory_maintenance_system === "string" ? agentPromptData.memory_maintenance_system : "",
          memoryMaintenanceUserTemplate: typeof agentPromptData.memory_maintenance_user_template === "string" ? agentPromptData.memory_maintenance_user_template : "",
          skillLearningSystem: typeof agentPromptData.skill_learning_system === "string" ? agentPromptData.skill_learning_system : "",
          skillLearningUserTemplate: typeof agentPromptData.skill_learning_user_template === "string" ? agentPromptData.skill_learning_user_template : ""
        });
      })
      .catch((error) => {
        if (cancelled) {
          return;
        }
        setPromptPreview({
          loading: false,
          error: getErrorMessage(error),
          statelessPrompt: "",
          memoryPrompt: "",
          memoryMaintenanceSystem: "",
          memoryMaintenanceUserTemplate: "",
          skillLearningSystem: "",
          skillLearningUserTemplate: ""
        });
      });

    return () => {
      cancelled = true;
    };
  }, [currentView, draftApiBase, draftMemoryUserId]);

  useEffect(() => {
    if (currentView !== "settings") {
      return;
    }

    let cancelled = false;
    setMcpPreview((current) => ({ ...current, loading: true, error: "" }));

    void fetchMcpPreview(draftApiBase, draftMcpConfigPath, draftMcpBaseUrls, draftMcpDisabledUrls, draftMcpLazyUrls)
      .then((data) => {
        if (cancelled) {
          return;
        }
        setMcpPreview({
          loading: false,
          error: "",
          servers: normalizeMcpPreviewServers(data.servers)
        });
      })
      .catch((error) => {
        if (cancelled) {
          return;
        }
        setMcpPreview({
          loading: false,
          error: getErrorMessage(error),
          servers: []
        });
      });

    return () => {
      cancelled = true;
    };
  }, [currentView, draftApiBase, draftMcpConfigPath, draftMcpBaseUrls, draftMcpDisabledUrls, draftMcpLazyUrls]);

  useEffect(() => {
    if (currentView === "skills") {
      void loadSkills();
    }
  }, [currentView, settings.apiBase, settings.memoryUserId, skillScope]);

  function showToast(message: string, tone: ToastItem["tone"]) {
    const id = createId("toast");
    setToasts((current) => [...current, { id, message, tone }]);
    window.setTimeout(() => {
      setToasts((current) => current.filter((item) => item.id !== id));
    }, 3000);
  }

  async function readError(response: Response) {
    try {
      const data = await response.json() as Record<string, unknown>;
      if (typeof data.detail === "string") {
        return data.detail;
      }
      return JSON.stringify(data);
    } catch {
      const text = await response.text();
      return text || `请求失败 (${response.status})`;
    }
  }

  async function fetchResponse(path: string, init?: RequestInit) {
    const response = await fetch(`${settings.apiBase}${path}`, init);
    if (!response.ok) {
      throw new Error(await readError(response));
    }
    setHealth((current) => (
      current.tone === "ok"
        ? current
        : { tone: "ok", label: "接口可达" }
    ));
    return response;
  }

  async function fetchJson(path: string, init?: RequestInit) {
    const response = await fetchResponse(path, init);
    return await response.json() as Record<string, unknown>;
  }

  async function testMcpConnection() {
    const data = await fetchMcpPreview(draftApiBase, draftMcpConfigPath, draftMcpBaseUrls, draftMcpDisabledUrls, draftMcpLazyUrls);
    const servers = normalizeMcpPreviewServers(data.servers);
    const { okCount, totalServers, totalTools } = summarizeMcpServers(servers);
    setMcpPreview({
      loading: false,
      error: "",
      servers
    });
    return { okCount, totalTools, totalServers };
  }

  async function copyMcpServer(server: McpServerPreview, mode: McpExposureMode) {
    await copyTextToClipboard(
      [
        `Endpoint: ${server.endpoint}`,
        `Mode: ${mode}`,
        `Status: ${server.ok ? "ok" : "error"}`,
        ...(server.error ? [`Error: ${server.error}`] : []),
        ...server.tools.map((tool) => `- ${tool.name}${tool.description ? ` - ${tool.description}` : ""}`)
      ].join("\n")
    );
    showToast(`已复制 ${server.endpoint} 的工具清单`, "success");
  }

  async function copyVisibleMcpTools(
    servers: Array<McpServerPreview & { tools: McpServerPreview["tools"] }>,
    disabledUrls: string[],
    lazyUrls: string[]
  ) {
    const lines = servers.flatMap((server) => {
      const mode = getMcpServerMode(server.endpoint, disabledUrls, lazyUrls);
      const base = [`Endpoint: ${server.endpoint} (${mode}, ${server.ok ? "ok" : "error"})`];
      if (server.error) {
        base.push(`Error: ${server.error}`);
      }
      if (server.tools.length) {
        base.push(...server.tools.map((tool) => `- ${tool.name}${tool.description ? ` - ${tool.description}` : ""}`));
      } else {
        base.push("- (no visible tools)");
      }
      return base;
    });

    if (!lines.length) {
      showToast("当前没有可复制的 MCP 工具", "info");
      return;
    }

    await copyTextToClipboard(lines.join("\n"));
    showToast(`已复制 ${servers.length} 个 MCP 的工具清单`, "success");
  }

  const skillUserId = settings.memoryUserId.trim() || DEFAULT_MEMORY_USER_ID;

  function buildSkillsPath(scope = skillScope) {
    const params = new URLSearchParams({ scope });
    if (scope === "private") {
      params.set("user_id", skillUserId);
    }
    return `/agent/skills?${params.toString()}`;
  }

  async function loadSkills(silent = true) {
    setSkillsLoading(true);
    try {
      const data = await fetchJson(buildSkillsPath());
      const items = Array.isArray(data.skills) ? (data.skills as SkillItem[]) : [];
      setSkills(items);
      setSkillEditor((editor) => {
        if (editor.mode === "edit" && editor.originalName) {
          const matched = items.find((item) => item.name === editor.originalName);
          return matched ? createEditorState(matched) : createEditorState();
        }

        if (editor.mode === "create") {
          return editor;
        }

        return items.length ? createEditorState(items[0]) : createEditorState();
      });
    } catch (error) {
      if (!silent) {
        showToast(getErrorMessage(error), "error");
      }
    } finally {
      setSkillsLoading(false);
    }
  }

  function replaceChats(updater: (current: Record<ChatModeId, ChatState>) => Record<ChatModeId, ChatState>) {
    setChats((current) => {
      const next = updater(current);
      chatsRef.current = next;
      return next;
    });
  }

  function updateChat(mode: ChatModeId, updater: (chat: ChatState) => ChatState) {
    replaceChats((current) => ({
      ...current,
      [mode]: updater(current[mode])
    }));
  }

  function patchMessage(mode: ChatModeId, messageId: string, updater: (message: DisplayMessage) => DisplayMessage) {
    updateChat(mode, (chat) => ({
      ...chat,
      messages: chat.messages.map((message) => message.id === messageId ? updater(message) : message)
    }));
  }

  function appendProcessItem(mode: ChatModeId, messageId: string, item: ProcessItem) {
    patchMessage(mode, messageId, (current) => ({
      ...current,
      processItems: [...current.processItems, item]
    }));
  }

  function takeNextQueuedSubmission(mode: ChatModeId) {
    let nextSubmission: QueuedChatSubmission | null = null;
    replaceChats((current) => {
      const [first, ...rest] = current[mode].queue;
      nextSubmission = first ?? null;
      if (!first) {
        return current;
      }
      return {
        ...current,
        [mode]: {
          ...current[mode],
          queue: rest
        }
      };
    });
    return nextSubmission;
  }

  function stopChat(mode: ChatModeId) {
    const controller = activeStreamControllersRef.current[mode];
    if (!controller || controller.signal.aborted) {
      return;
    }

    updateChat(mode, (current) => ({
      ...current,
      stopRequested: true
    }));
    controller.abort();
  }

  function sendChat(mode: ChatModeId) {
    const chat = chatsRef.current[mode];
    const message = chat.input.trim();

    if (!message) {
      return;
    }

    const submission: QueuedChatSubmission = {
      id: createId("queued"),
      message,
      files: [...chat.files]
    };

    if (chat.sending) {
      updateChat(mode, (current) => ({
        ...current,
        input: "",
        files: [],
        queue: [...current.queue, submission]
      }));
      return;
    }

    updateChat(mode, (current) => ({
      ...current,
      input: "",
      files: []
    }));

    void runChatSubmission(mode, submission);
  }

  async function runPreChatMcpHealthCheck() {
    const hasMcpSettings =
      settings.mcpConfigPath.trim().length > 0 || parseMcpBaseUrlsInput(settings.mcpBaseUrls).length > 0;
    if (!hasMcpSettings) {
      return null;
    }

    const cacheKey = buildMcpCacheKey(
      settings.apiBase,
      settings.mcpConfigPath,
      settings.mcpBaseUrls,
      settings.mcpDisabledUrls,
      settings.mcpLazyUrls
    );
    const now = Date.now();
    const cached = mcpHealthCacheRef.current;

    if (cached && cached.key === cacheKey && now - cached.checkedAt < MCP_HEALTH_CACHE_MS) {
      return cached;
    }

    try {
      const data = await fetchMcpPreview(
        settings.apiBase,
        settings.mcpConfigPath,
        settings.mcpBaseUrls,
        settings.mcpDisabledUrls,
        settings.mcpLazyUrls
      );
      const servers = normalizeMcpPreviewServers(data.servers);
      setMcpPreview({
        loading: false,
        error: "",
        servers
      });
      const result: McpHealthCache = {
        key: cacheKey,
        checkedAt: now,
        servers,
        error: null
      };
      mcpHealthCacheRef.current = result;
      const { okCount, totalServers, totalTools } = summarizeMcpServers(servers);
      showToast(
        totalServers
          ? `MCP 预检查：${okCount}/${totalServers} 可用，共 ${totalTools} 个工具`
          : "MCP 预检查：当前未解析到可用服务",
        okCount > 0 || totalServers === 0 ? "info" : "error"
      );
      return result;
    } catch (error) {
      const detail = getErrorMessage(error);
      setMcpPreview({
        loading: false,
        error: detail,
        servers: []
      });
      const result: McpHealthCache = {
        key: cacheKey,
        checkedAt: now,
        servers: [],
        error: detail
      };
      mcpHealthCacheRef.current = result;
      showToast(`MCP 预检查失败：${detail}`, "error");
      return result;
    }
  }

  async function runChatSubmission(mode: ChatModeId, submission: QueuedChatSubmission) {
    const config = CHAT_MODES[mode];
    const assistantId = createId("assistant");
    const userMessage: DisplayMessage = {
      id: createId("user"),
      role: "user",
      text: submission.message,
      attachments: submission.files.map((file) => file.name),
      processing: false,
      outputFiles: [],
      processItems: []
    };
    const assistantMessage: DisplayMessage = {
      id: assistantId,
      role: "assistant",
      text: "",
      attachments: [],
      processing: true,
      outputFiles: [],
      processItems: []
    };

    updateChat(mode, (current) => ({
      ...current,
      sending: true,
      stopRequested: false,
      messages: [...current.messages, userMessage, assistantMessage]
    }));

    try {
      await runPreChatMcpHealthCheck();
      const history = chatsRef.current[mode].history;
      let result: ChatTurnResult;

      if (config.streaming) {
        const controller = new AbortController();
        activeStreamControllersRef.current[mode] = controller;
        result = await sendStreamingChat(
          config,
          history,
          submission.message,
          submission.files,
          assistantId,
          controller.signal
        );
      } else {
        result = await sendSyncChat(config, history, submission.message, submission.files, assistantId);
      }

      if (!result.aborted || result.reply.trim()) {
        updateChat(mode, (current) => ({
          ...current,
          history: [
            ...current.history,
            { role: "user", content: submission.message },
            { role: "assistant", content: result.reply }
          ]
        }));
      }

      if (result.aborted) {
        showToast("已停止当前回答", "info");
      }
    } catch (error) {
      patchMessage(mode, assistantId, (current) => ({
        ...current,
        text: current.text
          ? `${current.text}\n\n[流式中断] ${getErrorMessage(error)}`
          : `错误：${getErrorMessage(error)}`,
        processing: false
      }));
      showToast(getErrorMessage(error), "error");
    } finally {
      activeStreamControllersRef.current[mode] = null;
      updateChat(mode, (current) => ({
        ...current,
        sending: false,
        stopRequested: false
      }));
      const nextSubmission = takeNextQueuedSubmission(mode);
      if (nextSubmission) {
        void runChatSubmission(mode, nextSubmission);
      }
    }
  }

  async function sendSyncChat(
    config: ChatModeConfig,
    history: HistoryEntry[],
    message: string,
    files: File[],
    assistantId: string
  ): Promise<ChatTurnResult> {
    const data = await fetchJson(config.endpoint, {
      method: "POST",
      body: buildFormData(config, history, message, files, settings)
    });

    const reply = String(data.reply || "");
    patchMessage(config.id, assistantId, (current) => ({
      ...current,
      text: reply,
      processing: false,
      outputFiles: Array.isArray(data.output_files) ? (data.output_files as OutputFile[]) : [],
      processItems: [
        ...(Array.isArray(data.skills_updated)
          ? [{ event: "skills_updated", count: data.skills_updated.length, skills: data.skills_updated }]
          : [])
      ]
    }));

    if (Array.isArray(data.skills_updated) && data.skills_updated.length > 0) {
      showToast(`已更新 ${data.skills_updated.length} 个私有 skill`, "success");
    }

    return { reply, aborted: false };
  }

  async function sendStreamingChat(
    config: ChatModeConfig,
    history: HistoryEntry[],
    message: string,
    files: File[],
    assistantId: string,
    signal: AbortSignal
  ): Promise<ChatTurnResult> {
    let fullReply = "";
    let pendingText = "";
    let revealTimer: number | null = null;
    let streamError: string | null = null;
    let finishReason = "stop";
    let aborted = false;

    const flushPendingText = () => {
      if (!pendingText) {
        return;
      }

      const step = getStreamRevealStep(pendingText.length);
      const nextSlice = pendingText.slice(0, step);
      pendingText = pendingText.slice(step);

      patchMessage(config.id, assistantId, (current) => ({
        ...current,
        text: current.text + nextSlice
      }));
    };

    const ensureRevealLoop = () => {
      if (revealTimer !== null) {
        return;
      }

      revealTimer = window.setInterval(() => {
        flushPendingText();
      }, STREAM_REVEAL_INTERVAL_MS);
    };

    const waitForRevealDrain = async () => {
      while (pendingText.length > 0) {
        await new Promise((resolve) => window.setTimeout(resolve, STREAM_REVEAL_INTERVAL_MS));
      }
    };

    try {
      const response = await fetchResponse(config.endpoint, {
        method: "POST",
        body: buildFormData(config, history, message, files, settings),
        signal
      });
      await readEventStream(response, (eventName, payload) => {
        if (eventName === "text" && typeof payload.text === "string") {
          fullReply += payload.text;
          pendingText += payload.text;
          ensureRevealLoop();
          return;
        }

        if ((eventName === "tool_use" || eventName === "tool_result") && payload) {
          appendProcessItem(config.id, assistantId, { event: eventName, ...payload });
          return;
        }

        if (eventName === "output_files" && Array.isArray(payload.files)) {
          patchMessage(config.id, assistantId, (current) => ({
            ...current,
            outputFiles: payload.files as OutputFile[]
          }));
          return;
        }

        if (eventName === "files_uploaded" && Array.isArray(payload.files)) {
          appendProcessItem(config.id, assistantId, {
            event: "files_uploaded",
            files: payload.files
          });
          return;
        }

        if (eventName === "skills_updated" && payload) {
          appendProcessItem(config.id, assistantId, { event: "skills_updated", ...payload });
          const count = typeof payload.count === "number" ? payload.count : 0;
          if (count > 0) {
            showToast(`已更新 ${count} 个私有 skill`, "success");
          }
          return;
        }

        if (eventName === "done") {
          finishReason = typeof payload.finish_reason === "string" ? payload.finish_reason : "stop";
          if (finishReason !== "stop") {
            appendProcessItem(config.id, assistantId, { event: "done", finish_reason: finishReason });
          }
          return;
        }

        if (eventName === "error") {
          streamError = typeof payload.detail === "string" ? payload.detail : "流式处理失败";
          appendProcessItem(config.id, assistantId, { event: "error", detail: streamError });
        }
      });
    } catch (error) {
      if (isAbortError(error)) {
        aborted = true;
        finishReason = "user_stopped";
      } else {
        throw error;
      }
    } finally {
      await waitForRevealDrain();
      if (revealTimer !== null) {
        window.clearInterval(revealTimer);
      }
    }

    if (streamError) {
      throw new Error(streamError);
    }

    if (aborted) {
      appendProcessItem(config.id, assistantId, { event: "done", finish_reason: finishReason });
    }

    patchMessage(config.id, assistantId, (current) => ({
      ...current,
      text: fullReply || (aborted ? "已停止当前回答。" : ""),
      processing: false
    }));

    if (!aborted && finishReason !== "stop") {
      showToast(`流式响应结束：${describeFinishReason(finishReason)}`, "info");
    }

    return { reply: fullReply, aborted };
  }

  async function saveSkill() {
    const draft = skillEditor.draft;
    const trimmedName = draft.name.trim();
    const trimmedFolder = draft.folder.trim();
    const trimmedBody = draft.body.trim();

    if (!trimmedName) {
      showToast("Skill 名称不能为空", "error");
      return;
    }
    if (skillEditor.mode === "create" && !trimmedFolder) {
      showToast("新建 Skill 时必须填写目录名", "error");
      return;
    }
    if (!trimmedBody) {
      showToast("Skill 正文不能为空", "error");
      return;
    }

    setSkillsSaving(true);
    try {
      const path = skillEditor.mode === "create"
        ? "/agent/skills"
        : `/agent/skills/${encodeURIComponent(skillEditor.originalName || "")}`;
      const method = skillEditor.mode === "create" ? "POST" : "PUT";
      const data = await fetchJson(path, {
        method,
        headers: {
          "Content-Type": "application/json"
        },
        body: JSON.stringify({
          name: trimmedName,
          description: draft.description.trim(),
          tags: draft.tags.trim(),
          trigger: draft.trigger.trim(),
          body: trimmedBody,
          folder: skillEditor.mode === "create" ? trimmedFolder : undefined,
          scope: skillScope,
          user_id: skillScope === "private" ? skillUserId : undefined
        })
      });

      const items = Array.isArray(data.skills) ? (data.skills as SkillItem[]) : [];
      setSkills(items);
      if (data.skill) {
        const savedSkill = data.skill as SkillItem;
        setSkillEditor(createEditorState(savedSkill));
        navigate(getSkillDetailPath(savedSkill.name));
      } else {
        setSkillEditor(items.length ? createEditorState(items[0]) : createEditorState());
        if (items.length) {
          navigate(getSkillDetailPath(items[0].name));
        }
      }
      showToast(skillEditor.mode === "create" ? "Skill 已创建" : "Skill 已更新", "success");
    } catch (error) {
      showToast(getErrorMessage(error), "error");
    } finally {
      setSkillsSaving(false);
    }
  }

  async function deleteSkill() {
    if (skillEditor.mode !== "edit" || !skillEditor.originalName || skillsDeleting) {
      return;
    }

    const skillName = skillEditor.originalName;
    const confirmed = window.confirm(`确认删除 skill "${skillName}" 吗？这会删除对应目录及其中内容。`);
    if (!confirmed) {
      return;
    }

    setSkillsDeleting(true);
    try {
      const params = new URLSearchParams({ scope: skillScope });
      if (skillScope === "private") {
        params.set("user_id", skillUserId);
      }
      const data = await fetchJson(`/agent/skills/${encodeURIComponent(skillName)}?${params.toString()}`, {
        method: "DELETE"
      });
      const items = Array.isArray(data.skills) ? (data.skills as SkillItem[]) : [];
      setSkills(items);

      if (items.length) {
        setSkillEditor(createEditorState(items[0]));
        navigate(getSkillDetailPath(items[0].name));
      } else {
        setSkillEditor(createEditorState());
        navigate(SKILL_CREATE_PATH);
      }

      showToast(`Skill 已删除：${skillName}`, "success");
    } catch (error) {
      showToast(getErrorMessage(error), "error");
    } finally {
      setSkillsDeleting(false);
    }
  }

  function renderChatWorkspace(mode: ChatModeId) {
    return (
      <ChatWorkspace
        chat={chats[mode]}
        config={CHAT_MODES[mode]}
        currentUserId={skillUserId}
        makeDownloadUrl={(file) => buildDownloadUrl(settings.apiBase, file)}
        onClear={() => replaceChats((current) => ({ ...current, [mode]: createInitialChats()[mode] }))}
        onInputChange={(value) => updateChat(mode, (current) => ({ ...current, input: value }))}
        onRemoveFile={(index) => updateChat(mode, (current) => ({
          ...current,
          files: current.files.filter((_, currentIndex) => currentIndex !== index)
        }))}
        onSelectFiles={(files) => updateChat(mode, (current) => ({
          ...current,
          files: [...current.files, ...files]
        }))}
        onSend={() => sendChat(mode)}
        onStop={() => stopChat(mode)}
      />
    );
  }

  const heading = getViewHeading(currentView);
  const isChatRoute = isChatView(currentView);

  return (
    <>
      <div className={`app-shell ${sidebarCollapsed ? "sidebar-collapsed" : ""}`}>
        <aside className={`sidebar ${sidebarCollapsed ? "collapsed" : ""}`}>
          <div className="sidebar-toolbar">
            <button
              aria-label={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
              className="sidebar-toggle"
              onClick={() => setSidebarCollapsed((current) => !current)}
              title={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
              type="button"
            >
              {sidebarCollapsed ? "»" : "«"}
            </button>
          </div>
          <div className="brand">
            <div className="brand-mark">CAS</div>
            <div className="brand-copy">
              <div className="brand-title">{settings.brandTitle}</div>
              <div className="brand-subtitle">{settings.brandSubtitle}</div>
            </div>
          </div>
          <div className="sidebar-nav">
            {NAV_GROUPS.map((group) => (
              <div className="nav-group" key={group.title}>
                <div className="nav-group-title">{group.title}</div>
                {group.items.map((item) => (
                  <button
                    className={`nav-button ${currentView === item.id ? "active" : ""}`}
                    key={item.id}
                    onClick={() => navigate(getPathForView(item.id))}
                    title={item.title}
                    type="button"
                  >
                    <span className="nav-icon">{getNavIcon(item.id)}</span>
                    <span className="nav-button-label">
                      <span className="nav-button-title">{item.title}</span>
                      <span className="nav-button-desc">{item.description}</span>
                    </span>
                    <span className="nav-pill">{item.pill}</span>
                  </button>
                ))}
              </div>
            ))}
          </div>
          <div className="sidebar-footer">
            <div className="sidebar-footer-block">
              <div className="sidebar-footer-label">当前后端地址</div>
              <div className="sidebar-footer-value">{settings.apiBase}</div>
            </div>
            <div className="sidebar-footer-block">
              <div className="sidebar-footer-label">当前用户 ID</div>
              <UserIdBadge tone="dark" value={skillUserId} />
            </div>
          </div>
        </aside>

        <main className={`main ${isChatRoute ? "main-chat" : ""}`}>
          {!isChatRoute ? (
            <div className="topbar">
              <div className="page-heading">
                <div className="soft-chip">{heading.eyebrow}</div>
                <h1>{heading.title}</h1>
                <p>{heading.description}</p>
              </div>
              <div className="topbar-side">
                <div className="status-stack">
                  <div className={`status-chip ${health.tone}`}>{health.label}</div>
                  <div className="meta-chip">{settings.apiBase}</div>
                </div>
                <div className="topbar-meta">设置页修改后端地址后，所有请求都会立即切换到新的 API 基地址。聊天记录和 skills 草稿会自动保存在浏览器本地。</div>
              </div>
            </div>
          ) : null}

          <div className={`view-root ${isChatRoute ? "chat-view-root" : ""}`}>
            <Routes>
              <Route element={<Navigate replace to={getPathForView("stream")} />} path="/" />
              <Route element={<Navigate replace to={getPathForView("stream")} />} path="/chat" />
              <Route element={<Navigate replace to={getPathForView("stream")} />} path="/chat/normal" />
              <Route element={renderChatWorkspace("stream")} path="/chat/stream" />
              <Route element={<Navigate replace to={getPathForView("memoryStream")} />} path="/chat/memory-run" />
              <Route element={renderChatWorkspace("memoryStream")} path="/chat/memory-stream" />
              <Route
                element={
                  <SkillsWorkspace
                    deleting={skillsDeleting}
                    editor={skillEditor}
                    items={skills}
                    loading={skillsLoading}
                    onCreate={() => {
                      setSkillEditor(createEditorState());
                      navigate(SKILL_CREATE_PATH);
                    }}
                    onEditorChange={setSkillEditor}
                    onReload={() => void loadSkills(false)}
                    onReset={() => {
                      if (skillEditor.mode === "edit" && skillEditor.originalName) {
                        const matched = skills.find((item) => item.name === skillEditor.originalName);
                        setSkillEditor(matched ? createEditorState(matched) : createEditorState());
                        return;
                      }
                      setSkillEditor(createEditorState());
                    }}
                    onDelete={() => void deleteSkill()}
                    onSave={() => void saveSkill()}
                    onScopeChange={(nextScope) => {
                      setSkillScope(nextScope);
                      setSkillEditor(createEditorState());
                      navigate("/skills");
                    }}
                    onSelect={(skill) => {
                      setSkillEditor(createEditorState(skill));
                      navigate(getSkillDetailPath(skill.name));
                    }}
                    scope={skillScope}
                    saving={skillsSaving}
                    userId={skillUserId}
                  />
                }
                path="/skills"
              />
              <Route
                element={
                  <SkillsWorkspace
                    deleting={skillsDeleting}
                    editor={skillEditor}
                    items={skills}
                    loading={skillsLoading}
                    onCreate={() => {
                      setSkillEditor(createEditorState());
                      navigate(SKILL_CREATE_PATH);
                    }}
                    onEditorChange={setSkillEditor}
                    onReload={() => void loadSkills(false)}
                    onReset={() => setSkillEditor(createEditorState())}
                    onDelete={() => void deleteSkill()}
                    onSave={() => void saveSkill()}
                    onScopeChange={(nextScope) => {
                      setSkillScope(nextScope);
                      setSkillEditor(createEditorState());
                      navigate("/skills");
                    }}
                    onSelect={(skill) => {
                      setSkillEditor(createEditorState(skill));
                      navigate(getSkillDetailPath(skill.name));
                    }}
                    scope={skillScope}
                    saving={skillsSaving}
                    userId={skillUserId}
                  />
                }
                path="/skills/new"
              />
              <Route
                element={
                  <SkillsWorkspace
                    deleting={skillsDeleting}
                    editor={skillEditor}
                    items={skills}
                    loading={skillsLoading}
                    onCreate={() => {
                      setSkillEditor(createEditorState());
                      navigate(SKILL_CREATE_PATH);
                    }}
                    onEditorChange={setSkillEditor}
                    onReload={() => void loadSkills(false)}
                    onReset={() => {
                      if (skillEditor.mode === "edit" && skillEditor.originalName) {
                        const matched = skills.find((item) => item.name === skillEditor.originalName);
                        setSkillEditor(matched ? createEditorState(matched) : createEditorState());
                        return;
                      }
                      setSkillEditor(createEditorState());
                    }}
                    onDelete={() => void deleteSkill()}
                    onSave={() => void saveSkill()}
                    onScopeChange={(nextScope) => {
                      setSkillScope(nextScope);
                      setSkillEditor(createEditorState());
                      navigate("/skills");
                    }}
                    onSelect={(skill) => {
                      setSkillEditor(createEditorState(skill));
                      navigate(getSkillDetailPath(skill.name));
                    }}
                    scope={skillScope}
                    saving={skillsSaving}
                    userId={skillUserId}
                  />
                }
                path="/skills/:skillName"
              />
              <Route
                element={
                  <SettingsWorkspace
                    apiBase={draftApiBase}
                    agentPromptAppend={draftAgentPromptAppend}
                    brandTitle={draftBrandTitle}
                    brandSubtitle={draftBrandSubtitle}
                    health={health}
                    mcpPreview={mcpPreview}
                    maxIterations={draftMaxIterations}
                    maxTokens={draftMaxTokens}
                    memoryMaintenanceSystemPrompt={draftMemoryMaintenanceSystemPrompt}
                    memoryMaintenanceUserPrompt={draftMemoryMaintenanceUserPrompt}
                    memoryUserId={draftMemoryUserId}
                    mcpConfigPath={draftMcpConfigPath}
                    mcpBaseUrls={draftMcpBaseUrls}
                    mcpDisabledUrls={draftMcpDisabledUrls}
                    mcpLazyUrls={draftMcpLazyUrls}
                    modelId={draftModelId}
                    promptPreview={promptPreview}
                    skillLearningSystemPrompt={draftSkillLearningSystemPrompt}
                    skillLearningUserPrompt={draftSkillLearningUserPrompt}
                    temperature={draftTemperature}
                    topP={draftTopP}
                    onApiBaseChange={setDraftApiBase}
                    onAgentPromptAppendChange={setDraftAgentPromptAppend}
                    onMaxIterationsChange={setDraftMaxIterations}
                    onMaxTokensChange={setDraftMaxTokens}
                    onBrandSubtitleChange={setDraftBrandSubtitle}
                    onBrandTitleChange={setDraftBrandTitle}
                    onMemoryMaintenanceSystemPromptChange={setDraftMemoryMaintenanceSystemPrompt}
                    onMemoryMaintenanceUserPromptChange={setDraftMemoryMaintenanceUserPrompt}
                    onMemoryUserIdChange={setDraftMemoryUserId}
                    onMcpConfigPathChange={setDraftMcpConfigPath}
                    onMcpBaseUrlsChange={setDraftMcpBaseUrls}
                    onMcpDisabledUrlsChange={setDraftMcpDisabledUrls}
                    onMcpLazyUrlsChange={setDraftMcpLazyUrls}
                    onModelIdChange={setDraftModelId}
                    onCopyMcpServer={(server, mode) => {
                      void copyMcpServer(server, mode).catch((error) => {
                        showToast(getErrorMessage(error), "error");
                      });
                    }}
                    onCopyVisibleMcpTools={(servers, disabledUrls, lazyUrls) => {
                      void copyVisibleMcpTools(servers, disabledUrls, lazyUrls).catch((error) => {
                        showToast(getErrorMessage(error), "error");
                      });
                    }}
                    onSkillLearningSystemPromptChange={setDraftSkillLearningSystemPrompt}
                    onSkillLearningUserPromptChange={setDraftSkillLearningUserPrompt}
                    onTestMcp={async () => {
                      try {
                        setMcpPreview((current) => ({ ...current, loading: true, error: "" }));
                        const result = await testMcpConnection();
                        showToast(`MCP 检测完成：${result.okCount}/${result.totalServers} 可用，共 ${result.totalTools} 个工具`, result.okCount > 0 ? "success" : "error");
                      } catch (error) {
                        setMcpPreview({
                          loading: false,
                          error: getErrorMessage(error),
                          servers: []
                        });
                        showToast(getErrorMessage(error), "error");
                      }
                    }}
                    onReset={() => {
                      setDraftApiBase(DEFAULT_SETTINGS.apiBase);
                      setDraftBrandTitle(DEFAULT_SETTINGS.brandTitle);
                      setDraftBrandSubtitle(DEFAULT_SETTINGS.brandSubtitle);
                      setDraftMemoryUserId(DEFAULT_SETTINGS.memoryUserId);
                      setDraftMcpConfigPath(DEFAULT_SETTINGS.mcpConfigPath);
                      setDraftMcpBaseUrls(DEFAULT_SETTINGS.mcpBaseUrls);
                      setDraftMcpDisabledUrls(DEFAULT_SETTINGS.mcpDisabledUrls);
                      setDraftMcpLazyUrls(DEFAULT_SETTINGS.mcpLazyUrls);
                      setDraftAgentPromptAppend(DEFAULT_SETTINGS.agentPromptAppend);
                      setDraftModelId(DEFAULT_SETTINGS.modelId);
                      setDraftTemperature(DEFAULT_SETTINGS.temperature);
                      setDraftMaxTokens(DEFAULT_SETTINGS.maxTokens);
                      setDraftMaxIterations(DEFAULT_SETTINGS.maxIterations);
                      setDraftTopP(DEFAULT_SETTINGS.topP);
                      setDraftMemoryMaintenanceSystemPrompt(DEFAULT_SETTINGS.memoryMaintenanceSystemPrompt);
                      setDraftMemoryMaintenanceUserPrompt(DEFAULT_SETTINGS.memoryMaintenanceUserPrompt);
                      setDraftSkillLearningSystemPrompt(DEFAULT_SETTINGS.skillLearningSystemPrompt);
                      setDraftSkillLearningUserPrompt(DEFAULT_SETTINGS.skillLearningUserPrompt);
                    }}
                    onSave={() => {
                      try {
                        const next = normalizeApiBase(draftApiBase || DEFAULT_SETTINGS.apiBase);
                        setSettings({
                          apiBase: next,
                          brandTitle: draftBrandTitle.trim() || DEFAULT_SETTINGS.brandTitle,
                          brandSubtitle: draftBrandSubtitle.trim() || DEFAULT_SETTINGS.brandSubtitle,
                          memoryUserId: draftMemoryUserId.trim() || DEFAULT_SETTINGS.memoryUserId,
                          mcpConfigPath: draftMcpConfigPath.trim(),
                          mcpBaseUrls: draftMcpBaseUrls.trim(),
                          mcpDisabledUrls: normalizeMcpUrlList(draftMcpDisabledUrls),
                          mcpLazyUrls: normalizeMcpUrlList(
                            draftMcpLazyUrls.filter((item) => !normalizeMcpUrlList(draftMcpDisabledUrls).includes(normalizeMcpEndpoint(item)))
                          ),
                          agentPromptAppend: draftAgentPromptAppend.trim(),
                          modelId: draftModelId.trim(),
                          temperature: normalizeOptionalNumericSetting(draftTemperature, "Temperature", "float", 0, 2),
                          maxTokens: normalizeOptionalNumericSetting(draftMaxTokens, "Max Tokens", "int", 1),
                          maxIterations: normalizeOptionalNumericSetting(draftMaxIterations, "Max Iterations", "int", 1),
                          topP: normalizeOptionalNumericSetting(draftTopP, "Top P", "float", 0.01, 1),
                          memoryMaintenanceSystemPrompt: draftMemoryMaintenanceSystemPrompt.trim(),
                          memoryMaintenanceUserPrompt: draftMemoryMaintenanceUserPrompt.trim(),
                          skillLearningSystemPrompt: draftSkillLearningSystemPrompt.trim(),
                          skillLearningUserPrompt: draftSkillLearningUserPrompt.trim()
                        });
                        showToast("设置已保存", "success");
                      } catch (error) {
                        showToast(getErrorMessage(error), "error");
                      }
                    }}
                    onTemperatureChange={setDraftTemperature}
                    onTest={async () => {
                      try {
                        const data = await testHealth(draftApiBase);
                        showToast(`连接成功：${String(data.model || "unknown")}`, "success");
                      } catch (error) {
                        showToast(getErrorMessage(error), "error");
                      }
                    }}
                    onTopPChange={setDraftTopP}
                  />
                }
                path="/settings"
              />
              <Route element={<Navigate replace to={getPathForView("stream")} />} path="*" />
            </Routes>
          </div>
        </main>
      </div>

      <div className="toast-layer">
        {toasts.map((toast) => (
          <div className={`toast ${toast.tone}`} key={toast.id}>
            {toast.message}
          </div>
        ))}
      </div>
    </>
  );
}

function ChatWorkspace(props: {
  config: ChatModeConfig;
  chat: ChatState;
  currentUserId: string;
  onInputChange: (value: string) => void;
  onSelectFiles: (files: File[]) => void;
  onRemoveFile: (index: number) => void;
  onSend: () => void;
  onStop: () => void;
  onClear: () => void;
  makeDownloadUrl: (file: OutputFile) => string;
}) {
  const { chat, config, currentUserId, onClear, onInputChange, onRemoveFile, onSelectFiles, onSend, onStop, makeDownloadUrl } = props;
  const threadEndRef = useRef<HTMLDivElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const isComposingRef = useRef(false);
  const scrollContainerRef = useRef<ScrollContainer | null>(null);
  const latestStreamingReplyRef = useRef<HTMLDivElement | null>(null);
  const shouldAutoFollowRef = useRef(true);
  const forceScrollRef = useRef(false);
  const initializedMessagesRef = useRef(false);
  const previousMessageCountRef = useRef(chat.messages.length);
  const turns = buildConversationTurns(chat.messages);
  const previousTurnCountRef = useRef(turns.length);
  const [showScrollToBottomButton, setShowScrollToBottomButton] = useState(false);
  const [hasUnreadUpdates, setHasUnreadUpdates] = useState(false);
  const [unreadTurnCount, setUnreadTurnCount] = useState(0);

  const scrollToBottom = (behavior: ScrollBehavior) => {
    const container = scrollContainerRef.current ?? resolveScrollContainer(threadEndRef.current);
    scrollContainerRef.current = container;
    if (container) {
      scrollContainerToBottom(container, behavior);
    } else {
      threadEndRef.current?.scrollIntoView({
        block: "end",
        behavior
      });
    }
    forceScrollRef.current = false;
    shouldAutoFollowRef.current = true;
    setShowScrollToBottomButton(false);
    setHasUnreadUpdates(false);
    setUnreadTurnCount(0);
  };

  useEffect(() => {
    const container = resolveScrollContainer(threadEndRef.current);
    scrollContainerRef.current = container;
    if (!container) {
      return;
    }

    const updateAutoFollow = () => {
      const distance = getDistanceToBottom(container);
      shouldAutoFollowRef.current = distance <= AUTO_SCROLL_BOTTOM_THRESHOLD_PX;
      setShowScrollToBottomButton(distance > SCROLL_TO_BOTTOM_BUTTON_THRESHOLD_PX);
      if (distance <= AUTO_SCROLL_BOTTOM_THRESHOLD_PX) {
        setHasUnreadUpdates(false);
        setUnreadTurnCount(0);
      }
    };

    updateAutoFollow();

    const handleScroll = () => {
      updateAutoFollow();
    };

    if (isWindowScrollContainer(container)) {
      window.addEventListener("scroll", handleScroll, { passive: true });
      window.addEventListener("resize", handleScroll);
      return () => {
        window.removeEventListener("scroll", handleScroll);
        window.removeEventListener("resize", handleScroll);
      };
    }

    container.addEventListener("scroll", handleScroll, { passive: true });
    window.addEventListener("resize", handleScroll);

    return () => {
      container.removeEventListener("scroll", handleScroll);
      window.removeEventListener("resize", handleScroll);
    };
  }, [config.id]);

  useEffect(() => {
    const container = scrollContainerRef.current ?? resolveScrollContainer(threadEndRef.current);
    scrollContainerRef.current = container;
    if (!container) {
      return;
    }

    const previousMessageCount = previousMessageCountRef.current;
    const currentMessageCount = chat.messages.length;
    const hasNewMessage = currentMessageCount > previousMessageCount;
    previousMessageCountRef.current = currentMessageCount;
    const previousTurnCount = previousTurnCountRef.current;
    const currentTurnCount = turns.length;
    const unreadTurnDelta = Math.max(0, currentTurnCount - previousTurnCount);
    previousTurnCountRef.current = currentTurnCount;

    if (!initializedMessagesRef.current) {
      initializedMessagesRef.current = true;
      shouldAutoFollowRef.current = isNearBottom(container);
      setHasUnreadUpdates(false);
      setUnreadTurnCount(0);
      return;
    }

    if (!forceScrollRef.current && !shouldAutoFollowRef.current) {
      if (unreadTurnDelta > 0) {
        setUnreadTurnCount((current) => current + unreadTurnDelta);
      } else {
        setHasUnreadUpdates(true);
      }
      return;
    }

    const frameId = window.requestAnimationFrame(() => {
      scrollToBottom(hasNewMessage ? "smooth" : "auto");
    });

    return () => window.cancelAnimationFrame(frameId);
  }, [chat.messages]);

  const handleSend = () => {
    if (!chat.input.trim()) {
      return;
    }

    forceScrollRef.current = true;
    shouldAutoFollowRef.current = true;
    setHasUnreadUpdates(false);
    setUnreadTurnCount(0);
    onSend();
  };

  const handleClear = () => {
    forceScrollRef.current = false;
    shouldAutoFollowRef.current = true;
    initializedMessagesRef.current = false;
    previousMessageCountRef.current = 0;
    previousTurnCountRef.current = 0;
    setShowScrollToBottomButton(false);
    setHasUnreadUpdates(false);
    setUnreadTurnCount(0);
    onClear();
  };

  const showUnreadAccent = hasScrollButtonUnreadAccent(hasUnreadUpdates, unreadTurnCount);
  const scrollButtonLabel = getScrollButtonLabel(hasUnreadUpdates, unreadTurnCount);
  const queuedPreview = chat.queue[0]?.message.trim() || "";
  const latestStreamingMessageId = [...chat.messages].reverse().find((message) => message.role === "assistant" && message.processing)?.id ?? null;

  useEffect(() => {
    if (!latestStreamingMessageId || (!forceScrollRef.current && !shouldAutoFollowRef.current)) {
      return;
    }

    const container = scrollContainerRef.current ?? resolveScrollContainer(threadEndRef.current);
    const streamingElement = latestStreamingReplyRef.current;
    scrollContainerRef.current = container;
    if (!container || !streamingElement) {
      return;
    }

    const frameId = window.requestAnimationFrame(() => {
      const { bottom: visibleBottom } = getScrollContainerViewportBounds(container);
      const targetBottom = streamingElement.getBoundingClientRect().bottom;

      if (isBottomWithinFollowThreshold(targetBottom, visibleBottom, STREAMING_VIEWPORT_FOLLOW_THRESHOLD_PX)) {
        return;
      }

      const followDelta = getViewportFollowDelta(
        targetBottom,
        visibleBottom,
        STREAMING_VIEWPORT_FOLLOW_MARGIN_PX
      );
      scrollContainerBy(container, followDelta, "auto");
    });

    return () => window.cancelAnimationFrame(frameId);
  }, [latestStreamingMessageId, chat.messages]);

  return (
    <div className="chat-page">
      <section className="chat-surface chat-surface-compact">
        <div className="chat-thread-shell">
          <div className="thread">
            {turns.length ? turns.map((turn) => (
              <ConversationTurnCard
                key={turn.id}
                latestStreamingMessageId={latestStreamingMessageId}
                latestStreamingReplyRef={latestStreamingReplyRef}
                makeDownloadUrl={makeDownloadUrl}
                turn={turn}
              />
            )) : (
              <div aria-hidden="true" className="thread-empty" />
            )}
            <div aria-hidden="true" ref={threadEndRef} />
          </div>
        </div>

        {showScrollToBottomButton ? (
          <button
            aria-label="回到最新消息"
            className={`scroll-bottom-button ${showUnreadAccent ? "has-unread" : ""}`}
            onClick={() => scrollToBottom("smooth")}
            type="button"
          >
            <span className="scroll-bottom-button-label">{scrollButtonLabel}</span>
            {unreadTurnCount > 0 ? (
              <>
                <span className="scroll-bottom-button-badge">{formatUnreadCount(unreadTurnCount)} 条</span>
              </>
            ) : hasUnreadUpdates ? <span aria-hidden="true" className="scroll-bottom-button-dot" /> : null}
          </button>
        ) : null}

        <div className="composer-shell">
          {config.memory ? (
            <div className="chat-context-bar">
              <span className="soft-chip">记忆用户</span>
              <UserIdBadge value={currentUserId} />
            </div>
          ) : null}
          <div className="composer">
            {chat.sending || chat.queue.length ? (
              <div className="composer-status-row">
                {chat.sending ? (
                  <span className={`soft-chip ${chat.stopRequested ? "soft-chip-attention" : ""}`}>
                    {chat.stopRequested ? "正在停止当前回答..." : "正在回答中，可继续提问"}
                  </span>
                ) : null}
                {chat.queue.length ? (
                  <span className="soft-chip">已排队 {chat.queue.length} 条，当前回答结束后自动继续</span>
                ) : null}
                {queuedPreview ? <span className="composer-queue-preview">下一条：{queuedPreview}</span> : null}
              </div>
            ) : null}
            <input
              accept=".doc,.docx,.csv,.xlsx,.xls,.txt,.pdf"
              hidden
              multiple
              onChange={(event) => {
                const files = Array.from(event.target.files || []);
                onSelectFiles(files);
                event.currentTarget.value = "";
              }}
              ref={fileInputRef}
              type="file"
            />

            {chat.files.length ? (
              <div className="file-row">
                {chat.files.map((file, index) => (
                  <span className="file-chip" key={`${file.name}-${index}`}>
                    {file.name} ({formatBytes(file.size)})
                    <button className="button ghost" onClick={() => onRemoveFile(index)} type="button">移除</button>
                  </span>
                ))}
              </div>
            ) : null}

            <textarea
              onChange={(event) => onInputChange(event.target.value)}
              onCompositionEnd={() => {
                isComposingRef.current = false;
              }}
              onCompositionStart={() => {
                isComposingRef.current = true;
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") {
                  return;
                }
                if (event.shiftKey) {
                  return;
                }
                if (isComposingRef.current || event.nativeEvent.isComposing || event.keyCode === 229) {
                  return;
                }
                event.preventDefault();
                handleSend();
              }}
              placeholder={config.placeholder}
              value={chat.input}
            />
            <div className="helper-text composer-hint">
              {chat.sending
                ? "`Enter` 可继续加入队列，`Shift + Enter` 换行；输入法联想期间不会误发"
                : "`Enter` 发送，`Shift + Enter` 换行；输入法联想期间不会误发"}
            </div>

            <div className="composer-actions">
              <div className="button-row">
                <button className="button secondary" onClick={() => fileInputRef.current?.click()} type="button">附件</button>
                <button className="button ghost" disabled={chat.sending} onClick={handleClear} type="button">清空</button>
              </div>
              <div className="button-row">
                {chat.sending ? (
                  <button className="button danger" disabled={chat.stopRequested} onClick={onStop} type="button">
                    {chat.stopRequested ? "停止中..." : "停止"}
                  </button>
                ) : null}
                <button className="button primary" disabled={!chat.input.trim()} onClick={handleSend} type="button">
                  发送
                </button>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}

function StreamingReply(props: {
  messageId: string;
  text: string;
  replyRef?: MutableRefObject<HTMLDivElement | null>;
}) {
  const { messageId, text, replyRef } = props;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const shouldAutoScrollRef = useRef(true);

  useEffect(() => {
    const element = containerRef.current;
    if (!element) {
      return;
    }

    shouldAutoScrollRef.current = isNearScrollableBottom(
      element.scrollHeight,
      element.scrollTop,
      element.clientHeight,
      STREAMING_REPLY_BOTTOM_THRESHOLD_PX
    );

    const handleScroll = () => {
      shouldAutoScrollRef.current = isNearScrollableBottom(
        element.scrollHeight,
        element.scrollTop,
        element.clientHeight,
        STREAMING_REPLY_BOTTOM_THRESHOLD_PX
      );
    };

    element.addEventListener("scroll", handleScroll, { passive: true });
    return () => {
      element.removeEventListener("scroll", handleScroll);
    };
  }, [messageId]);

  useEffect(() => {
    const element = containerRef.current;
    if (!element || !shouldAutoScrollRef.current) {
      return;
    }

    const frameId = window.requestAnimationFrame(() => {
      element.scrollTop = element.scrollHeight;
    });

    return () => window.cancelAnimationFrame(frameId);
  }, [text]);

  return (
    <div
      className="plain-text reply-streaming"
      dangerouslySetInnerHTML={{ __html: escapeWithLineBreaks(text) }}
      ref={(node) => {
        containerRef.current = node;
        if (replyRef) {
          replyRef.current = node;
        }
      }}
    />
  );
}

function renderUserMessageContent(message: DisplayMessage) {
  return (
    <>
      <div className="plain-text" dangerouslySetInnerHTML={{ __html: escapeWithLineBreaks(message.text) }} />
      {message.attachments.length ? (
        <div className="attachment-row">
          {message.attachments.map((file) => <span className="attachment-chip" key={file}>{file}</span>)}
        </div>
      ) : null}
    </>
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isAbortError(error: unknown) {
  return isRecord(error) && error.name === "AbortError";
}

function describeFinishReason(finishReason: string) {
  switch (finishReason) {
    case "max_iterations":
      return "达到最大工具/推理轮次上限";
    case "length":
      return "输出达到模型长度上限";
    case "user_stopped":
      return "用户手动停止";
    case "stop":
      return "正常结束";
    case "tool_calls":
      return "等待工具调用";
    case "empty":
      return "模型未返回有效内容";
    default:
      return finishReason;
  }
}

function renderProcessItem(item: ProcessItem, index: number) {
  if (isRecord(item) && item.event === "tool_use") {
    const title = typeof item.name === "string" && item.name.trim() ? item.name : "unknown";
    const argumentsText = typeof item.arguments === "string" ? item.arguments.trim() : "";
    return (
      <div className="process-item tool-use" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">TOOL</span>
          <strong>调用工具 · {title}</strong>
        </div>
        {argumentsText ? <pre>{argumentsText}</pre> : <div className="process-note">无参数</div>}
      </div>
    );
  }

  if (isRecord(item) && item.event === "tool_result") {
    const title = typeof item.tool === "string" && item.tool.trim() ? item.tool : "unknown";
    const outputText = typeof item.output === "string" ? item.output.trim() : "";
    return (
      <div className="process-item tool-result" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">RESULT</span>
          <strong>工具结果 · {title}</strong>
        </div>
        {outputText ? <pre>{outputText}</pre> : <div className="process-note">无输出</div>}
      </div>
    );
  }

  if (isRecord(item) && item.event === "files_uploaded" && Array.isArray(item.files)) {
    return (
      <div className="process-item files-uploaded" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">FILES</span>
          <strong>已上传 {item.files.length} 个文件</strong>
        </div>
        <div className="process-detail-list">
          {item.files.map((file, fileIndex) => (
            <div className="process-detail-row" key={`uploaded-${fileIndex}`}>
              <span>{isRecord(file) && typeof file.original_name === "string" ? file.original_name : "unknown"}</span>
              <span>{isRecord(file) && typeof file.size === "number" ? formatBytes(file.size) : ""}</span>
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (isRecord(item) && item.event === "skills_updated") {
    const count = typeof item.count === "number"
      ? item.count
      : Array.isArray(item.skills)
        ? item.skills.length
        : 0;
    const skillNames = Array.isArray(item.skills)
      ? item.skills
        .map((skill) => (isRecord(skill) && typeof skill.name === "string" ? skill.name : ""))
        .filter(Boolean)
      : [];
    return (
      <div className="process-item skills-updated" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">SKILL</span>
          <strong>已更新 {count} 个私有 Skill</strong>
        </div>
        {skillNames.length ? (
          <div className="process-chip-row">
            {skillNames.map((name) => <span className="process-chip" key={name}>{name}</span>)}
          </div>
        ) : (
          <div className="process-note">本轮无可展示的 skill 名称</div>
        )}
      </div>
    );
  }

  if (isRecord(item) && item.event === "done") {
    const finishReason = typeof item.finish_reason === "string" ? item.finish_reason : "stop";
    return (
      <div className="process-item done" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">DONE</span>
          <strong>流式完成</strong>
        </div>
        <div className="process-note">finish_reason: {finishReason} · {describeFinishReason(finishReason)}</div>
      </div>
    );
  }

  if (isRecord(item) && item.event === "error") {
    const detail = typeof item.detail === "string" ? item.detail : "未知错误";
    return (
      <div className="process-item error" key={`process-${index}`}>
        <div className="process-item-header">
          <span className="process-badge">ERROR</span>
          <strong>流式错误</strong>
        </div>
        <pre>{detail}</pre>
      </div>
    );
  }

  return (
    <div className="process-item raw" key={`process-${index}`}>
      <div className="process-item-header">
        <span className="process-badge">RAW</span>
        <strong>过程记录 {index + 1}</strong>
      </div>
      <pre>{JSON.stringify(item, null, 2)}</pre>
    </div>
  );
}

function renderAssistantMessageContent(
  message: DisplayMessage,
  makeDownloadUrl: (file: OutputFile) => string,
  streamingReplyRef?: MutableRefObject<HTMLDivElement | null>
) {
  const downloads = collectDownloadFiles(message);
  const streamingText = stripThinkingContent(message.text);
  const hasVisibleStreamingText = Boolean(streamingText.trim());
  const parsedParts = parseThinking(message.text);
  const thinkingParts = parsedParts
    .filter((part) => part.type === "thinking" && part.content.trim())
    .map((part) => part.content.trim());
  const hasThinkingActivity = thinkingParts.length > 0 || message.text.includes("<think>");
  const replyParts = parsedParts
    .filter((part) => part.type === "text" && part.content.trim())
    .map((part) => part.content);
  return (
    <>
      {message.processing ? (
        <>
          {hasThinkingActivity ? (
            <div className="fold-card thinking thinking-pending">
              <div className="thinking-status-header">
                <span>{thinkingParts.length > 1 ? `思考过程 (${thinkingParts.length} 段已记录)` : "思考过程"}</span>
                <span>运行中</span>
              </div>
              <div className="fold-card-body">
                <div className="processing">Agent 正在思考</div>
                <div className="thinking-status-note">思考内容会在本轮回答完成后折叠展示。</div>
              </div>
            </div>
          ) : null}
          {!hasThinkingActivity && !hasVisibleStreamingText ? <div className="processing">Agent 正在处理</div> : null}
          {hasVisibleStreamingText ? (
            <StreamingReply messageId={message.id} replyRef={streamingReplyRef} text={streamingText} />
          ) : null}
        </>
      ) : (
        <>
          {thinkingParts.length ? (
            <details className="fold-card thinking">
              <summary>
                <span>{thinkingParts.length > 1 ? `思考过程 (${thinkingParts.length} 步)` : "思考过程"}</span>
                <span>展开</span>
              </summary>
              <div className="fold-card-body thinking-steps">
                {thinkingParts.map((part, index) => (
                  <section className="thinking-step" key={`thinking-step-${index}`}>
                    {thinkingParts.length > 1 ? <div className="thinking-step-index">步骤 {index + 1}</div> : null}
                    <div className="reply" dangerouslySetInnerHTML={{ __html: renderMarkdown(part) }} />
                  </section>
                ))}
              </div>
            </details>
          ) : null}
          {replyParts.map((part, index) => (
            <div className="reply" dangerouslySetInnerHTML={{ __html: renderMarkdown(part) }} key={`reply-${index}`} />
          ))}
        </>
      )}
      {downloads.length ? (
        <div className="download-row">
          {downloads.map((file) => (
            <a className="download-link" href={makeDownloadUrl(file)} key={file.path || file.name} rel="noreferrer" target="_blank">
              下载 {file.name}
            </a>
          ))}
        </div>
      ) : null}
      {message.processItems.length ? (
        <details className="fold-card">
          <summary><span>处理过程 ({message.processItems.length} 步)</span><span>展开</span></summary>
          <div className="fold-card-body">
            <div className="process-list">
              {message.processItems.map((item, index) => renderProcessItem(item, index))}
            </div>
          </div>
        </details>
      ) : null}
    </>
  );
}

function ConversationTurnCard(props: {
  turn: { id: string; user?: DisplayMessage; assistant?: DisplayMessage };
  makeDownloadUrl: (file: OutputFile) => string;
  latestStreamingMessageId: string | null;
  latestStreamingReplyRef: MutableRefObject<HTMLDivElement | null>;
}) {
  const { turn, makeDownloadUrl, latestStreamingMessageId, latestStreamingReplyRef } = props;

  return (
    <article className="conversation-turn">
      {turn.user ? (
        <section className="dialog-row user">
          <div className="dialog-bubble user">
            <div className="dialog-label">你</div>
            {renderUserMessageContent(turn.user)}
          </div>
        </section>
      ) : null}
      {turn.assistant ? (
        <section className="dialog-row assistant">
          <div className="dialog-avatar">AI</div>
          <div className="dialog-bubble assistant">
            <div className="dialog-label">{turn.assistant.processing ? "回答生成中" : "智能体"}</div>
            {renderAssistantMessageContent(
              turn.assistant,
              makeDownloadUrl,
              turn.assistant.id === latestStreamingMessageId ? latestStreamingReplyRef : undefined
            )}
          </div>
        </section>
      ) : (
        <section className="dialog-row assistant">
          <div className="dialog-avatar">AI</div>
          <div className="dialog-bubble assistant">
            <div className="dialog-label">等待回复</div>
            <div className="processing">等待 Agent 响应</div>
          </div>
        </section>
      )}
    </article>
  );
}

function compactUserId(value: string) {
  const trimmed = value.trim();
  if (trimmed.length <= 18) {
    return trimmed;
  }
  return `${trimmed.slice(0, 8)}...${trimmed.slice(-8)}`;
}

function UserIdBadge(props: {
  value: string;
  tone?: "light" | "dark";
}) {
  const { tone = "light", value } = props;
  const [expanded, setExpanded] = useState(false);
  const trimmed = value.trim();
  const expandable = trimmed.length > 18;
  const displayValue = expanded || !expandable ? trimmed : compactUserId(trimmed);

  return (
    <button
      aria-expanded={expandable ? expanded : undefined}
      className={`user-id-badge user-id-badge-${tone} ${expanded ? "expanded" : ""}`}
      disabled={!expandable}
      onClick={() => {
        if (expandable) {
          setExpanded((current) => !current);
        }
      }}
      title={trimmed}
      type="button"
    >
      <span className="user-id-badge-value">{displayValue}</span>
      {expandable ? <span className="user-id-badge-toggle">{expanded ? "收起" : "展开"}</span> : null}
    </button>
  );
}

function SkillsWorkspace(props: {
  items: SkillItem[];
  scope: SkillScope;
  userId: string;
  loading: boolean;
  saving: boolean;
  deleting: boolean;
  editor: SkillEditorState;
  onSelect: (skill: SkillItem) => void;
  onCreate: () => void;
  onReload: () => void;
  onReset: () => void;
  onDelete: () => void;
  onSave: () => void;
  onScopeChange: (scope: SkillScope) => void;
  onEditorChange: (editor: SkillEditorState) => void;
}) {
  const { deleting, editor, items, loading, onCreate, onDelete, onEditorChange, onReload, onReset, onSave, onScopeChange, onSelect, saving, scope, userId } = props;
  const location = useLocation();
  const params = useParams<{ skillName: string }>();
  const isCreateRoute = location.pathname === SKILL_CREATE_PATH;
  const routedSkillName = params.skillName ? decodeURIComponent(params.skillName) : null;
  const [query, setQuery] = useState("");
  const scopeLabel = scope === "private" ? "用户私有" : "公用";
  const scopeDescription = scope === "private" ? `user_id: ${userId}` : "所有用户共享";

  const selectedSkill = routedSkillName
    ? items.find((item) => item.name === routedSkillName) || null
    : editor.mode === "edit" && editor.originalName
      ? items.find((item) => item.name === editor.originalName) || null
      : null;

  const filteredItems = items.filter((item) => {
    if (!query.trim()) {
      return true;
    }

    const haystack = [item.name, item.description, item.tags, item.trigger, item.folder]
      .join(" ")
      .toLowerCase();
    return haystack.includes(query.trim().toLowerCase());
  });

  const currentPath = editor.mode === "edit" && editor.originalName
    ? selectedSkill?.path || "未找到"
    : scope === "private"
      ? `.user_memories/${userId}/skills/${editor.draft.folder}/SKILL.md`
      : `skills/${editor.draft.folder}/SKILL.md`;
  const isDirty = isSkillEditorDirty(editor, selectedSkill);
  const bodyLines = editor.draft.body.split("\n").length;
  const bodyLength = editor.draft.body.trim().length;
  const routeLabel = isCreateRoute
    ? SKILL_CREATE_PATH
    : routedSkillName
      ? getSkillDetailPath(routedSkillName)
      : "/skills";

  useEffect(() => {
    if (isCreateRoute) {
      if (editor.mode !== "create" || editor.originalName !== null) {
        onEditorChange(createEditorState());
      }
      return;
    }

    if (selectedSkill && editor.originalName !== selectedSkill.name) {
      onEditorChange(createEditorState(selectedSkill));
    }
  }, [editor.mode, editor.originalName, isCreateRoute, onEditorChange, selectedSkill]);

  return (
    <div className="skills-grid">
      <section className="panel skills-list">
        <div className="section-head">
          <div>
            <h3>Skills</h3>
            <span>{scopeLabel} skills，当前共 {items.length} 个，可直接搜索、定位和编辑</span>
          </div>
          <div className="button-row">
            <button className={`button ${scope === "shared" ? "secondary" : "ghost"}`} onClick={() => onScopeChange("shared")} type="button">公用</button>
            <button className={`button ${scope === "private" ? "secondary" : "ghost"}`} onClick={() => onScopeChange("private")} type="button">私有</button>
            <button className="button secondary" disabled={loading} onClick={onReload} type="button">刷新</button>
            <button className="button primary" onClick={onCreate} type="button">新建 Skill</button>
          </div>
        </div>

        <label className="field">
          <span>搜索 Skill</span>
          <input onChange={(event) => setQuery(event.target.value)} placeholder="按名称、描述、tags 或 trigger 搜索" value={query} />
        </label>

        <div className="skills-summary-row">
          <span className="soft-chip">{scopeLabel}</span>
          <span className="soft-chip">{scopeDescription}</span>
          <span className="soft-chip">显示 {filteredItems.length} / {items.length}</span>
          {isCreateRoute ? <span className="soft-chip">创建模式</span> : null}
          {selectedSkill ? <span className="soft-chip">当前：{selectedSkill.name}</span> : null}
        </div>

        <div className="skills-scroll">
          {loading ? (
            <div className="empty-block">正在加载 skills 列表...</div>
          ) : filteredItems.length ? filteredItems.map((skill) => (
            <button
              className={`skill-card ${routedSkillName === skill.name || editor.originalName === skill.name ? "active" : ""}`}
              key={skill.name}
              onClick={() => onSelect(skill)}
              type="button"
            >
              <h4>{skill.name}</h4>
              <p>{skill.description || "暂无描述"}</p>
              <div className="skill-card-meta">
                <span className="soft-chip">{skill.scope === "private" ? "私有" : "公用"}</span>
                <span className="soft-chip">{skill.folder}</span>
                {skill.tags ? <span className="soft-chip">{skill.tags}</span> : null}
              </div>
            </button>
          )) : (
            <div className="empty-block">{items.length ? "没有匹配的 skill，试试换个关键词。" : "当前还没有 skill。点击右上角“新建 Skill”开始创建。"}</div>
          )}
        </div>
      </section>

      <section className="panel skill-editor">
        <div className="section-head">
          <div>
            <h3>{editor.mode === "create" ? "创建 Skill" : selectedSkill ? `编辑 ${selectedSkill.name}` : "Skills 工作台"}</h3>
            <span>{editor.mode === "create" ? `新建 ${scopeLabel} skill 会写入对应的 ${scope === "private" ? "用户目录" : "公用目录"}` : "修改会直接更新现有 skill 文件，右侧预览会实时刷新"}</span>
          </div>
          <div className="button-row">
            <span className={`soft-chip ${isDirty ? "soft-chip-attention" : ""}`}>{isDirty ? "有未保存改动" : "已同步"}</span>
            {editor.mode === "edit" && selectedSkill ? (
              <button className="button danger" disabled={saving || deleting} onClick={onDelete} type="button">
                {deleting ? "删除中..." : "删除"}
              </button>
            ) : null}
            <button className="button ghost" onClick={onReset} type="button">重置</button>
            <button className="button primary" disabled={saving || deleting} onClick={onSave} type="button">
              {saving ? "保存中..." : editor.mode === "create" ? "创建" : "保存修改"}
            </button>
          </div>
        </div>

        <div className="skills-meta-grid">
          <article className="meta-card">
            <strong>路由</strong>
            <span>{routeLabel}</span>
          </article>
          <article className="meta-card">
            <strong>文件</strong>
            <span>{currentPath}</span>
          </article>
          <article className="meta-card">
            <strong>正文规模</strong>
            <span>{bodyLines} 行 / {bodyLength} 字符</span>
          </article>
        </div>

        <div className="skills-workspace">
          <form className="skills-form" onSubmit={(event) => event.preventDefault()}>
            <div className="form-grid">
              <label className="field">
                <span>名称</span>
                <input
                  onChange={(event) => {
                    const nextName = event.target.value;
                    onEditorChange({
                      ...editor,
                      draft: {
                        ...editor.draft,
                        name: nextName,
                        folder: editor.mode === "create" && !editor.folderTouched ? suggestFolder(nextName || "new-skill") : editor.draft.folder
                      }
                    });
                  }}
                  placeholder="例如：generate_word"
                  value={editor.draft.name}
                />
              </label>

              <label className="field">
                <span>目录名</span>
                <input
                  disabled={editor.mode === "edit"}
                  onChange={(event) => onEditorChange({
                    ...editor,
                    folderTouched: true,
                    draft: {
                      ...editor.draft,
                      folder: event.target.value
                    }
                  })}
                  placeholder="例如：generate_word"
                  value={editor.draft.folder}
                />
                <small>仅新建时可配置，用于生成 skills/&lt;folder&gt;/SKILL.md。</small>
              </label>

              <label className="field">
                <span>描述</span>
                <input
                  onChange={(event) => onEditorChange({
                    ...editor,
                    draft: {
                      ...editor.draft,
                      description: event.target.value
                    }
                  })}
                  placeholder="一句话描述这个 skill 的作用"
                  value={editor.draft.description}
                />
              </label>

              <label className="field">
                <span>Tags</span>
                <input
                  onChange={(event) => onEditorChange({
                    ...editor,
                    draft: {
                      ...editor.draft,
                      tags: event.target.value
                    }
                  })}
                  placeholder="可选，逗号分隔"
                  value={editor.draft.tags}
                />
              </label>
            </div>

            <label className="field">
              <span>Trigger</span>
              <input
                onChange={(event) => onEditorChange({
                  ...editor,
                  draft: {
                    ...editor.draft,
                    trigger: event.target.value
                  }
                })}
                placeholder="可选，说明何时触发"
                value={editor.draft.trigger}
              />
            </label>

            <label className="field">
              <span>正文</span>
              <textarea
                onChange={(event) => onEditorChange({
                  ...editor,
                  draft: {
                    ...editor.draft,
                    body: event.target.value
                  }
                })}
                placeholder="填写 SKILL.md 正文内容"
                value={editor.draft.body}
              />
              <small>frontmatter 由界面字段生成，正文区域只编辑 markdown body。</small>
            </label>
          </form>

          <aside className="preview-pane">
            <div className="preview-card">
              <div className="section-head">
                <div>
                  <h3>元数据</h3>
                  <span>当前编辑内容的即时摘要</span>
                </div>
              </div>
              <div className="preview-metadata">
                <div className="preview-meta-item">
                  <strong>名称</strong>
                  <span>{editor.draft.name || "未命名 Skill"}</span>
                </div>
                <div className="preview-meta-item">
                  <strong>目录</strong>
                  <span>{editor.draft.folder || "-"}</span>
                </div>
                <div className="preview-meta-item">
                  <strong>描述</strong>
                  <span>{editor.draft.description || "暂无描述"}</span>
                </div>
                <div className="preview-meta-item">
                  <strong>Trigger</strong>
                  <span>{editor.draft.trigger || "未设置"}</span>
                </div>
              </div>
              <div className="skill-card-meta">
                {editor.draft.tags
                  ? editor.draft.tags.split(",").map((tag) => tag.trim()).filter(Boolean).map((tag) => <span className="soft-chip" key={tag}>{tag}</span>)
                  : <span className="helper-text">暂无 tags</span>}
              </div>
            </div>

            <div className="preview-card preview-markdown">
              <div className="section-head">
                <div>
                  <h3>Markdown 预览</h3>
                  <span>保存前即可看到最终内容结构</span>
                </div>
              </div>
              {editor.draft.body.trim() ? (
                <div className="reply" dangerouslySetInnerHTML={{ __html: renderMarkdown(editor.draft.body) }} />
              ) : (
                <div className="empty-block">正文为空，暂无预览内容。</div>
              )}
            </div>
          </aside>
        </div>

        {routedSkillName && !selectedSkill && !loading ? (
          <div className="empty-block">没有找到名为 “{routedSkillName}” 的 skill。你可以返回列表重新选择，或者新建一个新的 skill。</div>
        ) : null}
      </section>
    </div>
  );
}

function SettingsWorkspace(props: {
  apiBase: string;
  agentPromptAppend: string;
  brandTitle: string;
  brandSubtitle: string;
  health: HealthState;
  mcpPreview: McpPreviewState;
  memoryUserId: string;
  mcpConfigPath: string;
  mcpBaseUrls: string;
  mcpDisabledUrls: string[];
  mcpLazyUrls: string[];
  modelId: string;
  promptPreview: PromptPreviewState;
  temperature: string;
  maxTokens: string;
  maxIterations: string;
  topP: string;
  memoryMaintenanceSystemPrompt: string;
  memoryMaintenanceUserPrompt: string;
  skillLearningSystemPrompt: string;
  skillLearningUserPrompt: string;
  onApiBaseChange: (value: string) => void;
  onAgentPromptAppendChange: (value: string) => void;
  onBrandTitleChange: (value: string) => void;
  onBrandSubtitleChange: (value: string) => void;
  onMemoryUserIdChange: (value: string) => void;
  onMcpConfigPathChange: (value: string) => void;
  onMcpBaseUrlsChange: (value: string) => void;
  onMcpDisabledUrlsChange: (value: string[]) => void;
  onMcpLazyUrlsChange: (value: string[]) => void;
  onModelIdChange: (value: string) => void;
  onTestMcp: () => void;
  onTemperatureChange: (value: string) => void;
  onMaxTokensChange: (value: string) => void;
  onMaxIterationsChange: (value: string) => void;
  onTopPChange: (value: string) => void;
  onMemoryMaintenanceSystemPromptChange: (value: string) => void;
  onMemoryMaintenanceUserPromptChange: (value: string) => void;
  onSkillLearningSystemPromptChange: (value: string) => void;
  onSkillLearningUserPromptChange: (value: string) => void;
  onCopyMcpServer: (server: McpServerPreview, mode: McpExposureMode) => void;
  onCopyVisibleMcpTools: (
    servers: Array<McpServerPreview & { tools: McpServerPreview["tools"] }>,
    disabledUrls: string[],
    lazyUrls: string[]
  ) => void;
  onTest: () => void;
  onReset: () => void;
  onSave: () => void;
}) {
  const {
    apiBase,
    agentPromptAppend,
    brandTitle,
    brandSubtitle,
    health,
    mcpPreview,
    maxIterations,
    maxTokens,
    memoryMaintenanceSystemPrompt,
    memoryMaintenanceUserPrompt,
    memoryUserId,
    mcpConfigPath,
    mcpBaseUrls,
    mcpDisabledUrls,
    mcpLazyUrls,
    modelId,
    promptPreview,
    skillLearningSystemPrompt,
    skillLearningUserPrompt,
    temperature,
    topP,
    onApiBaseChange,
    onAgentPromptAppendChange,
    onMaxIterationsChange,
    onMaxTokensChange,
    onBrandSubtitleChange,
    onBrandTitleChange,
    onMemoryMaintenanceSystemPromptChange,
    onMemoryMaintenanceUserPromptChange,
    onMemoryUserIdChange,
    onMcpConfigPathChange,
    onMcpBaseUrlsChange,
    onMcpDisabledUrlsChange,
    onMcpLazyUrlsChange,
    onModelIdChange,
    onCopyMcpServer,
    onCopyVisibleMcpTools,
    onReset,
    onSave,
    onSkillLearningSystemPromptChange,
    onSkillLearningUserPromptChange,
    onTestMcp,
    onTemperatureChange,
    onTest,
    onTopPChange
  } = props;

  const [toolSearch, setToolSearch] = useState("");
  const [collapsedServers, setCollapsedServers] = useState<Record<string, boolean>>({});
  const normalizedSearch = toolSearch.trim().toLowerCase();
  const visibleServers = mcpPreview.servers
    .map((server) => {
      const tools = normalizedSearch
        ? server.tools.filter((tool) => {
          const haystack = `${tool.name} ${tool.description}`.toLowerCase();
          return haystack.includes(normalizedSearch);
        })
        : server.tools;
      return { ...server, tools, toolCount: server.toolCount };
    })
    .filter((server) => normalizedSearch ? server.tools.length > 0 || !server.ok : true);

  const visibleToolCount = visibleServers.reduce((sum, server) => sum + server.tools.length, 0);

  const setServerMode = (endpoint: string, mode: McpExposureMode) => {
    const normalized = normalizeMcpEndpoint(endpoint);
    const nextDisabled = normalizeMcpUrlList(
      mode === "disabled"
        ? [...mcpDisabledUrls.filter((item) => normalizeMcpEndpoint(item) !== normalized), normalized]
        : mcpDisabledUrls.filter((item) => normalizeMcpEndpoint(item) !== normalized)
    );
    const nextLazy = normalizeMcpUrlList(
      mode === "lazy"
        ? [...mcpLazyUrls.filter((item) => normalizeMcpEndpoint(item) !== normalized), normalized]
        : mcpLazyUrls.filter((item) => normalizeMcpEndpoint(item) !== normalized)
    );
    onMcpDisabledUrlsChange(nextDisabled);
    onMcpLazyUrlsChange(nextLazy);
  };

  const toggleServerCollapsed = (endpoint: string) => {
    setCollapsedServers((current) => ({
      ...current,
      [endpoint]: !current[endpoint]
    }));
  };

  return (
    <div className="settings-grid">
      <section className="panel settings-card">
        <div className="section-head">
          <div>
            <h3>基础设置</h3>
            <span>配置后立即作用于所有聊天请求和左上角品牌区展示。</span>
          </div>
        </div>
        <label className="field">
          <span>API Base URL</span>
          <input onChange={(event) => onApiBaseChange(event.target.value)} placeholder="http://localhost:8080" value={apiBase} />
          <small>建议填写完整协议和端口，例如 http://localhost:8080。</small>
        </label>
        <label className="field">
          <span>品牌标题</span>
          <input onChange={(event) => onBrandTitleChange(event.target.value)} placeholder="例如：中科院智能体平台" value={brandTitle} />
          <small>会显示在界面左上角品牌区主标题。</small>
        </label>
        <label className="field">
          <span>品牌描述</span>
          <textarea onChange={(event) => onBrandSubtitleChange(event.target.value)} placeholder="输入品牌区描述文案" value={brandSubtitle} />
          <small>会显示在品牌标题下方，用于概述平台定位。</small>
        </label>
        <label className="field">
          <span>默认用户 ID</span>
          <input onChange={(event) => onMemoryUserIdChange(event.target.value)} placeholder={DEFAULT_MEMORY_USER_ID} value={memoryUserId} />
          <small>记忆对话和私有 skills 共用这个 user_id；留空时会回退到默认值 {DEFAULT_MEMORY_USER_ID}。</small>
        </label>
        <label className="field">
          <span>MCP 配置文件路径</span>
          <input onChange={(event) => onMcpConfigPathChange(event.target.value)} placeholder="例如：/path/to/mcp.json" value={mcpConfigPath} />
          <small>可填写 MCP 配置文件路径，后端会从 mcpServers / servers 中读取多个服务地址。</small>
        </label>
        <label className="field">
          <span>MCP 服务地址列表</span>
          <textarea onChange={(event) => onMcpBaseUrlsChange(event.target.value)} placeholder={"每行或逗号一个地址，例如：\nhttp://localhost:8444/mcp\nhttp://localhost:8555/mcp"} value={mcpBaseUrls} />
          <small>支持一次连接多个 MCP；会和上面的配置文件路径一起生效。</small>
        </label>
        <div className="empty-block compact">
          每个 MCP 都可以单独设置为立即加载 / 按需加载 / 禁用。按需加载的 MCP 默认不会把全部工具暴露给模型，模型需要先搜索再按需激活。
        </div>
        <label className="field">
          <span>Agent 提示词追加项</span>
          <textarea onChange={(event) => onAgentPromptAppendChange(event.target.value)} placeholder="补充对主 agent 的长期指令，例如输出风格、回答约束、固定流程。" value={agentPromptAppend} />
          <small>这段内容会追加在后端默认系统提示词之后，不会覆盖现有默认规则。</small>
        </label>
        <div className="section-head">
          <div>
            <h3>记忆 / Skills 维护 Prompt</h3>
            <span>用于控制 agent 如何写入 USER.md、MEMORY.md，以及如何更新私有 skills。留空则回退后端默认模板。</span>
          </div>
        </div>
        <label className="field">
          <span>USER.md / MEMORY.md 维护 System Prompt</span>
          <textarea onChange={(event) => onMemoryMaintenanceSystemPromptChange(event.target.value)} placeholder="覆盖写入 USER.md / MEMORY.md 时使用的 system prompt" value={memoryMaintenanceSystemPrompt} />
          <small>建议只写行为约束，不要在这里塞当前会话内容。</small>
        </label>
        <label className="field">
          <span>USER.md / MEMORY.md 维护 User Prompt 模板</span>
          <textarea onChange={(event) => onMemoryMaintenanceUserPromptChange(event.target.value)} placeholder="覆盖写入 USER.md / MEMORY.md 时使用的 user prompt 模板" value={memoryMaintenanceUserPrompt} />
          <small>支持占位符：{"{user_limit}"}、{"{memory_limit}"}、{"{user_restructure}"}、{"{memory_restructure}"}、{"{user_md_path}"}、{"{memory_md_path}"}、{"{current_user_len}"}、{"{current_memory_len}"}、{"{user_message}"}、{"{assistant_reply}"}。</small>
        </label>
        <label className="field">
          <span>私有 Skills 学习 System Prompt</span>
          <textarea onChange={(event) => onSkillLearningSystemPromptChange(event.target.value)} placeholder="覆盖更新私有 skills 时使用的 system prompt" value={skillLearningSystemPrompt} />
          <small>用于约束 skill 学习方式，例如保守更新、强调结构稳定等。</small>
        </label>
        <label className="field">
          <span>私有 Skills 学习 User Prompt 模板</span>
          <textarea onChange={(event) => onSkillLearningUserPromptChange(event.target.value)} placeholder="覆盖更新私有 skills 时使用的 user prompt 模板" value={skillLearningUserPrompt} />
          <small>支持占位符：{"{source_scope}"}、{"{skill_path}"}、{"{skill_name}"}、{"{skill_body_len}"}、{"{user_message}"}、{"{assistant_reply}"}。</small>
        </label>
        <div className="section-head">
          <div>
            <h3>LLM 参数覆盖</h3>
            <span>留空时使用后端默认值；保存后会随每次聊天请求一起发送。</span>
          </div>
        </div>
        <div className="form-grid">
          <label className="field">
            <span>Model ID</span>
            <input onChange={(event) => onModelIdChange(event.target.value)} placeholder="例如：minimax-m2.5" value={modelId} />
            <small>模型标识，留空则沿用后端默认模型。</small>
          </label>
          <label className="field">
            <span>Max Tokens</span>
            <input onChange={(event) => onMaxTokensChange(event.target.value)} placeholder="例如：50000" value={maxTokens} />
            <small>正整数，控制单次回复的最大 token 上限。</small>
          </label>
          <label className="field">
            <span>Max Iterations</span>
            <input onChange={(event) => onMaxIterationsChange(event.target.value)} placeholder="例如：12" value={maxIterations} />
            <small>正整数，控制单次对话里模型最多能进行多少轮工具/推理迭代。</small>
          </label>
          <label className="field">
            <span>Temperature</span>
            <input onChange={(event) => onTemperatureChange(event.target.value)} placeholder="0 ~ 2，留空使用默认" value={temperature} />
            <small>数值越高越发散，越低越稳定。</small>
          </label>
          <label className="field">
            <span>Top P</span>
            <input onChange={(event) => onTopPChange(event.target.value)} placeholder="0.01 ~ 1，留空使用默认" value={topP} />
            <small>控制采样截断范围，通常和 temperature 二选一重点调节。</small>
          </label>
        </div>
        <div className="form-actions">
          <div className="button-row">
            <button className="button secondary" onClick={onTest} type="button">测试连接</button>
            <button className="button secondary" onClick={onTestMcp} type="button">测试 MCP</button>
            <button className="button ghost" onClick={onReset} type="button">恢复默认</button>
          </div>
          <button className="button primary" onClick={onSave} type="button">保存设置</button>
        </div>
      </section>

      <section className="panel settings-card">
        <h3>当前状态</h3>
        <div className="stats-grid">
          <article className="stat-card">
            <div className="soft-chip">Health</div>
            <h3>{health.label}</h3>
            <p>用于验证当前 API 地址是否可达。</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Current API</div>
            <h3>{apiBase}</h3>
            <p>所有界面都会复用这个地址发请求。</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Brand</div>
            <h3>{brandTitle || "未设置品牌标题"}</h3>
            <p>{brandSubtitle || "未设置品牌描述"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">User ID</div>
            <h3>{memoryUserId || DEFAULT_MEMORY_USER_ID}</h3>
            <p>记忆模式与私有 skills 使用的默认用户标识。</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">MCP</div>
            <h3>{mcpConfigPath.trim() || "未配置路径"}</h3>
            <p>{mcpBaseUrls.trim() ? `${parseMcpBaseUrlsInput(mcpBaseUrls).length} 个手动地址` : "未配置手动 MCP 地址"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">MCP Tools</div>
            <h3>{mcpPreview.servers.reduce((sum, server) => sum + server.toolCount, 0)}</h3>
            <p>{mcpPreview.servers.length ? `${mcpPreview.servers.filter((server) => server.ok).length}/${mcpPreview.servers.length} 个 MCP 可用` : "等待检测 MCP 服务"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">LLM Override</div>
            <h3>{modelId || "后端默认模型"}</h3>
            <p>Temp {temperature || "默认"} · Max {maxTokens || "默认"} · Iter {maxIterations || "默认"} · Top P {topP || "默认"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Prompt Addendum</div>
            <h3>{agentPromptAppend.trim() ? "已配置" : "未配置"}</h3>
            <p>{agentPromptAppend.trim() ? "会追加到默认系统提示词后面" : "当前仅使用后端默认系统提示词"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Memory Prompts</div>
            <h3>{memoryMaintenanceSystemPrompt.trim() || memoryMaintenanceUserPrompt.trim() ? "已覆盖" : "后端默认"}</h3>
            <p>{memoryMaintenanceSystemPrompt.trim() || memoryMaintenanceUserPrompt.trim() ? "USER.md / MEMORY.md 维护提示词已自定义" : "当前使用后端内置的 USER.md / MEMORY.md 维护模板"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Skill Prompts</div>
            <h3>{skillLearningSystemPrompt.trim() || skillLearningUserPrompt.trim() ? "已覆盖" : "后端默认"}</h3>
            <p>{skillLearningSystemPrompt.trim() || skillLearningUserPrompt.trim() ? "私有 Skill 学习提示词已自定义" : "当前使用后端内置的私有 Skill 学习模板"}</p>
          </article>
          <article className="stat-card">
            <div className="soft-chip">Storage</div>
            <h3>LocalStorage</h3>
            <p>设置、聊天记录和 skills 草稿都保存在当前浏览器。</p>
          </article>
        </div>
        <div className="empty-block">如果你把前端部署到独立域名，目标后端需要允许对应的 CORS 来源；否则浏览器会阻止跨域请求。</div>
      </section>

      <section className="panel settings-card">
        <div className="section-head">
          <div>
            <h3>MCP 连接与工具预览</h3>
            <span>可直接查看当前解析出的 MCP 服务列表、连接结果和工具清单。</span>
          </div>
          <div className="button-row">
            <button
              className="button ghost"
              onClick={() => setCollapsedServers(
                Object.fromEntries(mcpPreview.servers.map((server) => [server.endpoint, true]))
              )}
              type="button"
            >
              全部收起
            </button>
            <button
              className="button ghost"
              onClick={() => setCollapsedServers({})}
              type="button"
            >
              全部展开
            </button>
            <button
              className="button secondary"
              disabled={!visibleServers.length}
              onClick={() => onCopyVisibleMcpTools(visibleServers, mcpDisabledUrls, mcpLazyUrls)}
              type="button"
            >
              复制当前工具清单
            </button>
          </div>
        </div>
        <div className="mcp-toolbar">
          <label className="field mcp-search-field">
            <span>工具搜索</span>
            <input
              onChange={(event) => setToolSearch(event.target.value)}
              placeholder="按工具名或描述过滤"
              value={toolSearch}
            />
          </label>
          <div className="mcp-toolbar-meta">
            当前显示 {visibleServers.length} 个 MCP，{visibleToolCount} 个工具
          </div>
        </div>
        {mcpPreview.loading ? <div className="processing">正在检测 MCP 服务</div> : null}
        {mcpPreview.error ? <div className="empty-block">MCP 检测失败：{mcpPreview.error}</div> : null}
        {!mcpPreview.loading && !mcpPreview.error && !mcpPreview.servers.length ? (
          <div className="empty-block">当前还没有解析到 MCP 服务。你可以填写 MCP 配置路径，或在上方手动输入多个 MCP 地址。</div>
        ) : null}
        {!mcpPreview.loading && visibleServers.length ? (
          <div className="mcp-preview-list">
            {visibleServers.map((server) => {
              const mode = getMcpServerMode(server.endpoint, mcpDisabledUrls, mcpLazyUrls);
              const collapsed = Boolean(collapsedServers[server.endpoint]);
              return (
                <article className="mcp-preview-card" key={server.endpoint}>
                  <div className="mcp-preview-head">
                    <div>
                      <h4>{server.endpoint}</h4>
                      <p>
                        {mode === "disabled"
                          ? `已禁用 · ${server.ok ? `${server.toolCount} 个工具` : "连接失败"}`
                          : mode === "lazy"
                            ? server.ok
                              ? `按需加载 · ${server.toolCount} 个工具`
                              : "按需加载 · 连接失败"
                            : server.ok
                              ? `立即加载 · ${server.toolCount} 个工具`
                              : "立即加载 · 连接失败"}
                      </p>
                    </div>
                    <div className="mcp-card-actions">
                      <span className={`soft-chip ${server.ok ? "" : "soft-chip-error"}`}>{server.ok ? "OK" : "ERROR"}</span>
                      <div className="mcp-mode-switch" role="group" aria-label={`MCP mode for ${server.endpoint}`}>
                        {(["eager", "lazy", "disabled"] as McpExposureMode[]).map((candidate) => (
                          <button
                            className={`button ${mode === candidate ? "secondary" : "ghost"}`}
                            key={candidate}
                            onClick={() => setServerMode(server.endpoint, candidate)}
                            type="button"
                          >
                            {candidate === "eager" ? "立即" : candidate === "lazy" ? "按需" : "禁用"}
                          </button>
                        ))}
                      </div>
                      <button
                        className="button ghost"
                        onClick={() => toggleServerCollapsed(server.endpoint)}
                        type="button"
                      >
                        {collapsed ? "展开" : "收起"}
                      </button>
                      <button
                        className="button ghost"
                        onClick={() => onCopyMcpServer(server, mode)}
                        type="button"
                      >
                        复制
                      </button>
                    </div>
                  </div>
                  {server.error ? <div className="empty-block compact">{server.error}</div> : null}
                  {!collapsed && server.tools.length ? (
                    <div className="mcp-tool-list">
                      {server.tools.map((tool) => (
                        <div className="mcp-tool-item" key={`${server.endpoint}-${tool.name}`}>
                          <strong>{tool.name}</strong>
                          <span>{tool.description || "无描述"}</span>
                        </div>
                      ))}
                    </div>
                  ) : !collapsed && server.ok ? (
                    <div className="empty-block compact">该 MCP 当前没有返回工具。</div>
                  ) : null}
                </article>
              );
            })}
          </div>
        ) : null}
        {!mcpPreview.loading && !visibleServers.length && mcpPreview.servers.length ? (
          <div className="empty-block">当前筛选条件下没有匹配的 MCP 工具。</div>
        ) : null}
      </section>

      <section className="panel settings-card">
        <div className="section-head">
          <div>
            <h3>当前默认提示词预览</h3>
            <span>展示后端当前生成的系统提示词。记忆模式会基于默认用户 ID 合并 USER.md 和 MEMORY.md。</span>
          </div>
        </div>
        {promptPreview.loading ? <div className="processing">正在加载提示词预览</div> : null}
        {promptPreview.error ? <div className="empty-block">提示词预览加载失败：{promptPreview.error}</div> : null}
        {!promptPreview.loading && !promptPreview.error ? (
          <div className="form-grid">
            <label className="field">
              <span>无痕流式默认提示词</span>
              <textarea readOnly value={promptPreview.statelessPrompt} />
            </label>
            <label className="field">
              <span>记忆流式默认提示词</span>
              <textarea readOnly value={promptPreview.memoryPrompt} />
            </label>
          </div>
        ) : null}
      </section>

      <section className="panel settings-card">
        <div className="section-head">
          <div>
            <h3>后端默认维护 Prompt</h3>
            <span>展示后端当前内置的 USER.md / MEMORY.md 维护模板与私有 skill 学习模板，便于你复制后微调成覆盖项。</span>
          </div>
        </div>
        {promptPreview.loading ? <div className="processing">正在加载维护 Prompt</div> : null}
        {promptPreview.error ? <div className="empty-block">维护 Prompt 加载失败：{promptPreview.error}</div> : null}
        {!promptPreview.loading && !promptPreview.error ? (
          <div className="form-grid">
            <label className="field">
              <span>Memory 维护 System Prompt</span>
              <textarea readOnly value={promptPreview.memoryMaintenanceSystem} />
            </label>
            <label className="field">
              <span>Memory 维护 User Prompt 模板</span>
              <textarea readOnly value={promptPreview.memoryMaintenanceUserTemplate} />
            </label>
            <label className="field">
              <span>Skill 学习 System Prompt</span>
              <textarea readOnly value={promptPreview.skillLearningSystem} />
            </label>
            <label className="field">
              <span>Skill 学习 User Prompt 模板</span>
              <textarea readOnly value={promptPreview.skillLearningUserTemplate} />
            </label>
          </div>
        ) : null}
      </section>
    </div>
  );
}

async function testHealth(apiBase: string) {
  const response = await fetch(`${normalizeApiBase(apiBase || DEFAULT_SETTINGS.apiBase)}/health`);
  if (!response.ok) {
    throw new Error(await readErrorResponse(response));
  }
  return await response.json() as Record<string, unknown>;
}

async function fetchPromptPreview(apiBase: string, userId: string) {
  const target = normalizeApiBase(apiBase || DEFAULT_SETTINGS.apiBase);
  const params = new URLSearchParams();
  const trimmedUserId = userId.trim() || DEFAULT_MEMORY_USER_ID;
  if (trimmedUserId) {
    params.set("user_id", trimmedUserId);
  }

  const response = await fetch(`${target}/agent/system-prompt?${params.toString()}`);
  if (!response.ok) {
    throw new Error(await readErrorResponse(response));
  }

  return await response.json() as Record<string, unknown>;
}

async function fetchAgentPromptSettings(apiBase: string) {
  const target = normalizeApiBase(apiBase || DEFAULT_SETTINGS.apiBase);
  const response = await fetch(`${target}/agent/settings/prompts`);
  if (!response.ok) {
    throw new Error(await readErrorResponse(response));
  }
  return await response.json() as Record<string, unknown>;
}

async function fetchMcpPreview(
  apiBase: string,
  configPath: string,
  rawBaseUrls: string,
  disabledUrls: string[] = [],
  lazyUrls: string[] = []
) {
  const target = normalizeApiBase(apiBase || DEFAULT_SETTINGS.apiBase);
  const params = new URLSearchParams();
  const trimmedConfigPath = configPath.trim();
  if (trimmedConfigPath) {
    params.set("config_path", trimmedConfigPath);
  }
  const baseUrls = parseMcpBaseUrlsInput(rawBaseUrls);
  if (baseUrls.length) {
    params.set("base_urls", JSON.stringify(baseUrls));
  }
  const normalizedDisabled = normalizeMcpUrlList(disabledUrls);
  if (normalizedDisabled.length) {
    params.set("disabled_urls", JSON.stringify(normalizedDisabled));
  }
  const normalizedLazy = normalizeMcpUrlList(
    lazyUrls.filter((item) => !normalizedDisabled.includes(normalizeMcpEndpoint(item)))
  );
  if (normalizedLazy.length) {
    params.set("lazy_urls", JSON.stringify(normalizedLazy));
  }

  const query = params.toString();
  const response = await fetch(`${target}/agent/settings/mcp${query ? `?${query}` : ""}`);
  if (!response.ok) {
    throw new Error(await readErrorResponse(response));
  }
  return await response.json() as Record<string, unknown>;
}

async function readErrorResponse(response: Response) {
  let message = `请求失败 (${response.status})`;
  try {
    const data = await response.json() as Record<string, unknown>;
    if (typeof data.detail === "string") {
      message = data.detail;
    }
  } catch {
    const text = await response.text();
    if (text) {
      message = text;
    }
  }
  return message;
}
