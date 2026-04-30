export type ViewId = "stream" | "memoryStream" | "skills" | "settings";
export type ChatModeId = "stream" | "memoryStream";
export type ToastTone = "info" | "success" | "error";
export type SkillScope = "shared" | "private" | "effective";

export type HistoryEntry = {
  role: "user" | "assistant";
  content: string;
};

export type OutputFile = {
  name: string;
  path?: string;
};

export type UploadedStreamFile = {
  original_name: string;
  saved_path: string;
  size: number;
  content_type?: string | null;
};

export type ProcessItem =
  | {
    event: "tool_use";
    name?: string;
    arguments?: string;
  }
  | {
    event: "tool_result";
    tool?: string;
    output?: string;
  }
  | {
    event: "files_uploaded";
    files?: UploadedStreamFile[];
  }
  | {
    event: "skills_updated";
    count?: number;
    skills?: Array<{ name?: string; scope?: SkillScope | string }>;
  }
  | {
    event: "done";
    finish_reason?: string;
  }
  | {
    event: "error";
    detail?: string;
  }
  | Record<string, unknown>;

export type DisplayMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  attachments: string[];
  processing: boolean;
  outputFiles: OutputFile[];
  processItems: ProcessItem[];
};

export type QueuedChatSubmission = {
  id: string;
  message: string;
  files: File[];
};

export type ChatState = {
  history: HistoryEntry[];
  messages: DisplayMessage[];
  files: File[];
  input: string;
  sending: boolean;
  stopRequested: boolean;
  queue: QueuedChatSubmission[];
};

export type HealthState = {
  tone: "loading" | "ok" | "error";
  label: string;
};

export type AppSettings = {
  apiBase: string;
  brandTitle: string;
  brandSubtitle: string;
  memoryUserId: string;
  agentPromptAppend: string;
  modelId: string;
  temperature: string;
  maxTokens: string;
  maxIterations: string;
  topP: string;
  memoryMaintenanceSystemPrompt: string;
  memoryMaintenanceUserPrompt: string;
  skillLearningSystemPrompt: string;
  skillLearningUserPrompt: string;
};

export type SkillItem = {
  name: string;
  description: string;
  tags: string;
  trigger: string;
  folder: string;
  path: string;
  body: string;
  meta: Record<string, string>;
  scope?: SkillScope;
};

export type SkillDraft = {
  name: string;
  description: string;
  tags: string;
  trigger: string;
  body: string;
  folder: string;
};

export type SkillEditorState = {
  mode: "create" | "edit";
  originalName: string | null;
  folderTouched: boolean;
  draft: SkillDraft;
};

export type ChatModeConfig = {
  id: ChatModeId;
  view: ViewId;
  title: string;
  subtitle: string;
  placeholder: string;
  endpoint: string;
  streaming: boolean;
  memory: boolean;
  sessionId: string;
  userId?: string;
  badges: string[];
  suggestions: string[];
};

export type NavItem = {
  id: ViewId;
  title: string;
  description: string;
  pill: string;
};

export type ToastItem = {
  id: string;
  tone: ToastTone;
  message: string;
};
