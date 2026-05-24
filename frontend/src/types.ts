export type ViewId = "stream" | "memoryStream" | "harness" | "skills" | "settings";
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
    event: "steering";
    message?: string;
    skipped_tools?: string[];
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
  steeringPending: boolean;
  steeringPreview: string;
  queue: QueuedChatSubmission[];
};

export type HealthState = {
  tone: "loading" | "ok" | "error";
  label: string;
};

export type McpExposureMode = "eager" | "lazy" | "disabled";

export type AppSettings = {
  apiBase: string;
  brandTitle: string;
  brandSubtitle: string;
  memoryUserId: string;
  mcpConfigPath: string;
  mcpBaseUrls: string;
  mcpDisabledUrls: string[];
  mcpLazyUrls: string[];
  agentPromptOverride: string;
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

export type McpToolPreview = {
  name: string;
  description: string;
};

export type McpServerPreview = {
  endpoint: string;
  endpointKey: string;
  mode: McpExposureMode;
  toolCount: number;
  ok: boolean;
  error?: string;
  tools: McpToolPreview[];
};

export type PromptSourceKind = "builtin" | "file" | "request" | "none";

export type PromptSource = {
  kind: PromptSourceKind;
  path?: string | null;
};

export type HarnessSnapshotSurface = {
  key: string;
  source: PromptSource;
  sha1: string;
  bytes: number;
  content: string;
};

export type HarnessSnapshot = {
  snapshotId: string;
  generatedAtMs: number;
  memoryOnlySelfEvolution: boolean;
  surfaces: HarnessSnapshotSurface[];
};

export type HarnessToolSignal = {
  name: string;
  count: number;
};

export type HarnessCandidateSignal = {
  key: string;
  severity: string;
  summary: string;
  traceIds: string[];
};

export type HarnessSignalSummary = {
  inspectedTraces: number;
  memoryTraces: number;
  statelessTraces: number;
  successTraces: number;
  errorTraces: number;
  finalReplyRecoveredTraces: number;
  maxIterationsTraces: number;
  selfEvolutionExecutedTraces: number;
  avgIterations: number;
  avgToolCalls: number;
  topTools: HarnessToolSignal[];
  candidateSignals: HarnessCandidateSignal[];
  recentTraceIds: string[];
  memoryOnlySelfEvolution: boolean;
};

export type HarnessTraceRequest = {
  sessionId?: string | null;
  userIdPresent: boolean;
  historyItems: number;
  uploadedFiles: number;
  memorySnapshotInjected: boolean;
  selfEvolutionAllowed: boolean;
  resolvedModelId: string;
  resolvedMaxIterations: number;
  temperature?: number | null;
  topP?: number | null;
  mcpBaseUrls: number;
  mcpDisabledUrls: number;
  mcpLazyUrls: number;
};

export type HarnessTracePrompts = {
  topLevelSystem: PromptSource;
  systemAppend: PromptSource;
  finalAnswerRecovery: PromptSource;
  subagentShared: PromptSource;
  subagentExplore: PromptSource;
  subagentGeneral: PromptSource;
  memoryMaintenanceSystem: PromptSource;
  memoryMaintenanceUserTemplate: PromptSource;
  skillLearningSystem: PromptSource;
  skillLearningUserTemplate: PromptSource;
};

export type HarnessTraceOutcome = {
  status: string;
  finishReason: string;
  error?: string | null;
  iterations: number;
  toolCalls: number;
  toolNames: string[];
  replyChars: number;
  outputFiles: number;
  outputFileNames: string[];
  usedSkillNames: string[];
  skillsUpdated: number;
  finalReplyRecovered: boolean;
  selfEvolutionExecuted: boolean;
};

export type HarnessRunTrace = {
  traceId: string;
  harnessSnapshotId: string;
  startedAtMs: number;
  finishedAtMs: number;
  runKind: string;
  mode: string;
  request: HarnessTraceRequest;
  prompts: HarnessTracePrompts;
  outcome: HarnessTraceOutcome;
};

export type HarnessDecisionStatus = "proposed" | "accepted" | "rejected";

export type HarnessDecisionRecord = {
  decisionId: string;
  createdAtMs: number;
  title: string;
  summary: string;
  rationale: string;
  expectedImpact: string[];
  changedSurfaces: string[];
  validationPlan: string[];
  modeScope: string;
  status: HarnessDecisionStatus;
  relatedTraceIds: string[];
  snapshotBeforeId?: string | null;
  snapshotAfterId?: string | null;
};

export type HarnessDecisionDraft = {
  draftId: string;
  signalKey: string;
  severity: string;
  title: string;
  summary: string;
  rationale: string;
  expectedImpact: string[];
  changedSurfaces: string[];
  validationPlan: string[];
  modeScope: string;
  recommendedStatus: HarnessDecisionStatus;
  relatedTraceIds: string[];
  snapshotBeforeId?: string | null;
};

export type HarnessApprovalStatus = "approved" | "reverted";

export type HarnessApprovalChange = {
  surfaceKey: string;
  path: string;
  changed: boolean;
  beforeSha1: string;
  afterSha1: string;
  beforeBytes: number;
  afterBytes: number;
  byteDelta: number;
  beforeLines: number;
  afterLines: number;
  lineDelta: number;
  beforeContent: string;
  afterContent: string;
};

export type HarnessApprovalRecord = {
  approvalId: string;
  createdAtMs: number;
  decisionId?: string | null;
  title: string;
  summary: string;
  approvedBy: string;
  approvalNote?: string | null;
  modeScope: string;
  relatedTraceIds: string[];
  snapshotBeforeId: string;
  snapshotAfterId: string;
  changedSurfaces: HarnessApprovalChange[];
  runtimeReloaded: boolean;
  status: HarnessApprovalStatus;
  revertedFromApprovalId?: string | null;
};

export type HarnessApplyPreviewSurface = {
  surfaceKey: string;
  path: string;
  changed: boolean;
  beforeSha1: string;
  afterSha1: string;
  beforeBytes: number;
  afterBytes: number;
  byteDelta: number;
  beforeLines: number;
  afterLines: number;
  lineDelta: number;
  beforeContent: string;
  afterContent: string;
};

export type HarnessApplyPreview = {
  snapshotBefore: HarnessSnapshot;
  expectedSnapshotId?: string | null;
  changedSurfaceCount: number;
  surfaces: HarnessApplyPreviewSurface[];
};

export type SharedFrontendSettings = {
  brandTitle: string;
  brandSubtitle: string;
  mcpConfigPath: string;
  mcpBaseUrls: string;
  mcpDisabledUrls: string[];
  mcpLazyUrls: string[];
  agentPromptOverride: string;
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
