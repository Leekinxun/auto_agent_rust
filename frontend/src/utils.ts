import { marked } from "marked";
import type {
  AppSettings,
  ChatModeConfig,
  ChatModeId,
  ChatState,
  DisplayMessage,
  HarnessApplyPreview,
  HarnessApplyPreviewSurface,
  HarnessApprovalChange,
  HarnessApprovalRecord,
  HarnessCandidateSignal,
  HarnessDecisionDraft,
  HarnessDecisionRecord,
  HarnessRunTrace,
  HarnessSignalSummary,
  HarnessSnapshot,
  HarnessSnapshotSurface,
  SharedFrontendSettings,
  McpExposureMode,
  HitlDefaultAction,
  HitlRiskLevel,
  HitlRule,
  McpServerPreview,
  OutputFile,
  PromptSource,
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
      { id: "harness", title: "Harness 观测", description: "快照、trace、signals", pill: "AHE" },
      { id: "skills", title: "Skills 管理", description: "新增、编辑、刷新", pill: "CRUD" },
      { id: "settings", title: "设置", description: "配置后端地址", pill: "API" }
    ]
  }
];

export const VIEW_PATHS: Record<ViewId, string> = {
  stream: "/chat/stream",
  memoryStream: "/chat/memory-stream",
  harness: "/harness",
  skills: "/skills",
  settings: "/settings"
};

export const SKILL_CREATE_PATH = "/skills/new";

export function createInitialChats(): Record<ChatModeId, ChatState> {
  return {
    stream: {
      history: [],
      messages: [],
      files: [],
      input: "",
      sending: false,
      stopRequested: false,
      steeringPending: false,
      steeringPreview: "",
      queue: []
    },
    memoryStream: {
      history: [],
      messages: [],
      files: [],
      input: "",
      sending: false,
      stopRequested: false,
      steeringPending: false,
      steeringPreview: "",
      queue: []
    }
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
  if (normalized.startsWith("/harness")) {
    return "harness";
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

function normalizePromptSource(value: unknown): PromptSource {
  if (!isRecord(value)) {
    return { kind: "none" };
  }
  const rawKind = typeof value.kind === "string" ? value.kind : "none";
  return {
    kind:
      rawKind === "builtin" || rawKind === "file" || rawKind === "request" || rawKind === "none"
        ? rawKind
        : "none",
    path: typeof value.path === "string" ? value.path : null
  };
}

export function normalizeHarnessSnapshot(value: unknown): HarnessSnapshot | null {
  if (!isRecord(value) || typeof value.snapshot_id !== "string") {
    return null;
  }
  const surfaces: HarnessSnapshotSurface[] = Array.isArray(value.surfaces)
    ? value.surfaces.flatMap((surface) => {
      if (!isRecord(surface) || typeof surface.key !== "string") {
        return [];
      }
      return [{
        key: surface.key,
        source: normalizePromptSource(surface.source),
        sha1: typeof surface.sha1 === "string" ? surface.sha1 : "",
        bytes: typeof surface.bytes === "number" ? surface.bytes : 0,
        content: typeof surface.content === "string" ? surface.content : ""
      }];
    })
    : [];

  return {
    snapshotId: value.snapshot_id,
    generatedAtMs: typeof value.generated_at_ms === "number" ? value.generated_at_ms : 0,
    memoryOnlySelfEvolution: Boolean(value.memory_only_self_evolution),
    surfaces
  };
}

export function normalizeHarnessSignals(value: unknown): HarnessSignalSummary | null {
  if (!isRecord(value)) {
    return null;
  }

  const topTools = Array.isArray(value.top_tools)
    ? value.top_tools.flatMap((tool) => {
      if (!isRecord(tool) || typeof tool.name !== "string") {
        return [];
      }
      return [{
        name: tool.name,
        count: typeof tool.count === "number" ? tool.count : 0
      }];
    })
    : [];

  const candidateSignals: HarnessCandidateSignal[] = Array.isArray(value.candidate_signals)
    ? value.candidate_signals.flatMap((signal) => {
      if (!isRecord(signal) || typeof signal.key !== "string") {
        return [];
      }
      return [{
        key: signal.key,
        severity: typeof signal.severity === "string" ? signal.severity : "low",
        summary: typeof signal.summary === "string" ? signal.summary : "",
        traceIds: Array.isArray(signal.trace_ids)
          ? signal.trace_ids.filter((item): item is string => typeof item === "string")
          : []
      }];
    })
    : [];

  return {
    inspectedTraces: typeof value.inspected_traces === "number" ? value.inspected_traces : 0,
    memoryTraces: typeof value.memory_traces === "number" ? value.memory_traces : 0,
    statelessTraces: typeof value.stateless_traces === "number" ? value.stateless_traces : 0,
    successTraces: typeof value.success_traces === "number" ? value.success_traces : 0,
    errorTraces: typeof value.error_traces === "number" ? value.error_traces : 0,
    finalReplyRecoveredTraces:
      typeof value.final_reply_recovered_traces === "number" ? value.final_reply_recovered_traces : 0,
    maxIterationsTraces:
      typeof value.max_iterations_traces === "number" ? value.max_iterations_traces : 0,
    selfEvolutionExecutedTraces:
      typeof value.self_evolution_executed_traces === "number" ? value.self_evolution_executed_traces : 0,
    avgIterations: typeof value.avg_iterations === "number" ? value.avg_iterations : 0,
    avgToolCalls: typeof value.avg_tool_calls === "number" ? value.avg_tool_calls : 0,
    topTools,
    candidateSignals,
    recentTraceIds: Array.isArray(value.recent_trace_ids)
      ? value.recent_trace_ids.filter((item): item is string => typeof item === "string")
      : [],
    memoryOnlySelfEvolution: Boolean(value.memory_only_self_evolution)
  };
}

export function normalizeHarnessTraces(value: unknown): HarnessRunTrace[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((trace) => {
    if (!isRecord(trace) || typeof trace.trace_id !== "string") {
      return [];
    }
    const request = isRecord(trace.request) ? trace.request : {};
    const prompts = isRecord(trace.prompts) ? trace.prompts : {};
    const outcome = isRecord(trace.outcome) ? trace.outcome : {};

    return [{
      traceId: trace.trace_id,
      harnessSnapshotId: typeof trace.harness_snapshot_id === "string" ? trace.harness_snapshot_id : "",
      startedAtMs: typeof trace.started_at_ms === "number" ? trace.started_at_ms : 0,
      finishedAtMs: typeof trace.finished_at_ms === "number" ? trace.finished_at_ms : 0,
      runKind: typeof trace.run_kind === "string" ? trace.run_kind : "",
      mode: typeof trace.mode === "string" ? trace.mode : "",
      request: {
        sessionId: typeof request.session_id === "string" ? request.session_id : null,
        userIdPresent: Boolean(request.user_id_present),
        historyItems: typeof request.history_items === "number" ? request.history_items : 0,
        uploadedFiles: typeof request.uploaded_files === "number" ? request.uploaded_files : 0,
        memorySnapshotInjected: Boolean(request.memory_snapshot_injected),
        selfEvolutionAllowed: Boolean(request.self_evolution_allowed),
        resolvedModelId: typeof request.resolved_model_id === "string" ? request.resolved_model_id : "",
        resolvedMaxIterations:
          typeof request.resolved_max_iterations === "number" ? request.resolved_max_iterations : 0,
        temperature: typeof request.temperature === "number" ? request.temperature : null,
        topP: typeof request.top_p === "number" ? request.top_p : null,
        mcpBaseUrls: typeof request.mcp_base_urls === "number" ? request.mcp_base_urls : 0,
        mcpDisabledUrls:
          typeof request.mcp_disabled_urls === "number" ? request.mcp_disabled_urls : 0,
        mcpLazyUrls: typeof request.mcp_lazy_urls === "number" ? request.mcp_lazy_urls : 0
      },
      prompts: {
        topLevelSystem: normalizePromptSource(prompts.top_level_system),
        systemAppend: normalizePromptSource(prompts.system_append),
        finalAnswerRecovery: normalizePromptSource(prompts.final_answer_recovery),
        subagentShared: normalizePromptSource(prompts.subagent_shared),
        subagentExplore: normalizePromptSource(prompts.subagent_explore),
        subagentGeneral: normalizePromptSource(prompts.subagent_general),
        memoryMaintenanceSystem: normalizePromptSource(prompts.memory_maintenance_system),
        memoryMaintenanceUserTemplate: normalizePromptSource(prompts.memory_maintenance_user_template),
        skillLearningSystem: normalizePromptSource(prompts.skill_learning_system),
        skillLearningUserTemplate: normalizePromptSource(prompts.skill_learning_user_template)
      },
      outcome: {
        status: typeof outcome.status === "string" ? outcome.status : "",
        finishReason: typeof outcome.finish_reason === "string" ? outcome.finish_reason : "",
        error: typeof outcome.error === "string" ? outcome.error : null,
        iterations: typeof outcome.iterations === "number" ? outcome.iterations : 0,
        toolCalls: typeof outcome.tool_calls === "number" ? outcome.tool_calls : 0,
        toolNames: Array.isArray(outcome.tool_names)
          ? outcome.tool_names.filter((item): item is string => typeof item === "string")
          : [],
        replyChars: typeof outcome.reply_chars === "number" ? outcome.reply_chars : 0,
        outputFiles: typeof outcome.output_files === "number" ? outcome.output_files : 0,
        outputFileNames: Array.isArray(outcome.output_file_names)
          ? outcome.output_file_names.filter((item): item is string => typeof item === "string")
          : [],
        usedSkillNames: Array.isArray(outcome.used_skill_names)
          ? outcome.used_skill_names.filter((item): item is string => typeof item === "string")
          : [],
        skillsUpdated: typeof outcome.skills_updated === "number" ? outcome.skills_updated : 0,
        finalReplyRecovered: Boolean(outcome.final_reply_recovered),
        selfEvolutionExecuted: Boolean(outcome.self_evolution_executed)
      }
    }];
  });
}

function normalizeHarnessDecisionStatus(value: unknown) {
  return value === "accepted" || value === "rejected" ? value : "proposed";
}

export function normalizeHarnessDecisions(value: unknown): HarnessDecisionRecord[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((decision) => {
    if (!isRecord(decision) || typeof decision.decision_id !== "string") {
      return [];
    }
    return [{
      decisionId: decision.decision_id,
      createdAtMs: typeof decision.created_at_ms === "number" ? decision.created_at_ms : 0,
      title: typeof decision.title === "string" ? decision.title : "",
      summary: typeof decision.summary === "string" ? decision.summary : "",
      rationale: typeof decision.rationale === "string" ? decision.rationale : "",
      expectedImpact: Array.isArray(decision.expected_impact)
        ? decision.expected_impact.filter((item): item is string => typeof item === "string")
        : [],
      changedSurfaces: Array.isArray(decision.changed_surfaces)
        ? decision.changed_surfaces.filter((item): item is string => typeof item === "string")
        : [],
      validationPlan: Array.isArray(decision.validation_plan)
        ? decision.validation_plan.filter((item): item is string => typeof item === "string")
        : [],
      modeScope: typeof decision.mode_scope === "string" ? decision.mode_scope : "",
      status: normalizeHarnessDecisionStatus(decision.status),
      relatedTraceIds: Array.isArray(decision.related_trace_ids)
        ? decision.related_trace_ids.filter((item): item is string => typeof item === "string")
        : [],
      snapshotBeforeId: typeof decision.snapshot_before_id === "string" ? decision.snapshot_before_id : null,
      snapshotAfterId: typeof decision.snapshot_after_id === "string" ? decision.snapshot_after_id : null
    }];
  });
}

export function normalizeHarnessDrafts(value: unknown): HarnessDecisionDraft[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((draft) => {
    if (!isRecord(draft) || typeof draft.draft_id !== "string") {
      return [];
    }
    return [{
      draftId: draft.draft_id,
      signalKey: typeof draft.signal_key === "string" ? draft.signal_key : "",
      severity: typeof draft.severity === "string" ? draft.severity : "low",
      title: typeof draft.title === "string" ? draft.title : "",
      summary: typeof draft.summary === "string" ? draft.summary : "",
      rationale: typeof draft.rationale === "string" ? draft.rationale : "",
      expectedImpact: Array.isArray(draft.expected_impact)
        ? draft.expected_impact.filter((item): item is string => typeof item === "string")
        : [],
      changedSurfaces: Array.isArray(draft.changed_surfaces)
        ? draft.changed_surfaces.filter((item): item is string => typeof item === "string")
        : [],
      validationPlan: Array.isArray(draft.validation_plan)
        ? draft.validation_plan.filter((item): item is string => typeof item === "string")
        : [],
      modeScope: typeof draft.mode_scope === "string" ? draft.mode_scope : "",
      recommendedStatus: normalizeHarnessDecisionStatus(draft.recommended_status),
      relatedTraceIds: Array.isArray(draft.related_trace_ids)
        ? draft.related_trace_ids.filter((item): item is string => typeof item === "string")
        : [],
      snapshotBeforeId: typeof draft.snapshot_before_id === "string" ? draft.snapshot_before_id : null
    }];
  });
}


function normalizeHarnessApprovalStatus(value: unknown): "approved" | "reverted" {
  return value === "reverted" ? "reverted" : "approved";
}

function normalizeHarnessApprovalChange(value: unknown): HarnessApprovalChange | null {
  if (!isRecord(value) || typeof value.surface_key !== "string") {
    return null;
  }
  return {
    surfaceKey: value.surface_key,
    path: typeof value.path === "string" ? value.path : "",
    changed: Boolean(value.changed),
    beforeSha1: typeof value.before_sha1 === "string" ? value.before_sha1 : "",
    afterSha1: typeof value.after_sha1 === "string" ? value.after_sha1 : "",
    beforeBytes: typeof value.before_bytes === "number" ? value.before_bytes : 0,
    afterBytes: typeof value.after_bytes === "number" ? value.after_bytes : 0,
    byteDelta: typeof value.byte_delta === "number" ? value.byte_delta : 0,
    beforeLines: typeof value.before_lines === "number" ? value.before_lines : 0,
    afterLines: typeof value.after_lines === "number" ? value.after_lines : 0,
    lineDelta: typeof value.line_delta === "number" ? value.line_delta : 0,
    beforeContent: typeof value.before_content === "string" ? value.before_content : "",
    afterContent: typeof value.after_content === "string" ? value.after_content : ""
  };
}

export function normalizeHarnessApprovals(value: unknown): HarnessApprovalRecord[] {
  if (!Array.isArray(value)) {
    return [];
  }

  return value.flatMap((approval) => {
    if (!isRecord(approval) || typeof approval.approval_id !== "string") {
      return [];
    }
    const changedSurfaces = Array.isArray(approval.changed_surfaces)
      ? approval.changed_surfaces
        .map((item) => normalizeHarnessApprovalChange(item))
        .filter((item): item is HarnessApprovalChange => item !== null)
      : [];
    return [{
      approvalId: approval.approval_id,
      createdAtMs: typeof approval.created_at_ms === "number" ? approval.created_at_ms : 0,
      decisionId: typeof approval.decision_id === "string" ? approval.decision_id : null,
      title: typeof approval.title === "string" ? approval.title : "",
      summary: typeof approval.summary === "string" ? approval.summary : "",
      approvedBy: typeof approval.approved_by === "string" ? approval.approved_by : "",
      approvalNote: typeof approval.approval_note === "string" ? approval.approval_note : null,
      modeScope: typeof approval.mode_scope === "string" ? approval.mode_scope : "",
      relatedTraceIds: Array.isArray(approval.related_trace_ids)
        ? approval.related_trace_ids.filter((item): item is string => typeof item === "string")
        : [],
      snapshotBeforeId: typeof approval.snapshot_before_id === "string" ? approval.snapshot_before_id : "",
      snapshotAfterId: typeof approval.snapshot_after_id === "string" ? approval.snapshot_after_id : "",
      changedSurfaces,
      runtimeReloaded: Boolean(approval.runtime_reloaded),
      status: normalizeHarnessApprovalStatus(approval.status),
      revertedFromApprovalId:
        typeof approval.reverted_from_approval_id === "string" ? approval.reverted_from_approval_id : null
    }];
  });
}

function normalizeHarnessApplyPreviewSurface(value: unknown): HarnessApplyPreviewSurface | null {
  if (!isRecord(value) || typeof value.surface_key !== "string") {
    return null;
  }
  return {
    surfaceKey: value.surface_key,
    path: typeof value.path === "string" ? value.path : "",
    changed: Boolean(value.changed),
    beforeSha1: typeof value.before_sha1 === "string" ? value.before_sha1 : "",
    afterSha1: typeof value.after_sha1 === "string" ? value.after_sha1 : "",
    beforeBytes: typeof value.before_bytes === "number" ? value.before_bytes : 0,
    afterBytes: typeof value.after_bytes === "number" ? value.after_bytes : 0,
    byteDelta: typeof value.byte_delta === "number" ? value.byte_delta : 0,
    beforeLines: typeof value.before_lines === "number" ? value.before_lines : 0,
    afterLines: typeof value.after_lines === "number" ? value.after_lines : 0,
    lineDelta: typeof value.line_delta === "number" ? value.line_delta : 0,
    beforeContent: typeof value.before_content === "string" ? value.before_content : "",
    afterContent: typeof value.after_content === "string" ? value.after_content : ""
  };
}

export function normalizeHarnessApplyPreview(value: unknown): HarnessApplyPreview | null {
  if (!isRecord(value)) {
    return null;
  }
  const snapshotBefore = normalizeHarnessSnapshot(value.snapshot_before);
  if (!snapshotBefore) {
    return null;
  }
  const surfaces = Array.isArray(value.surfaces)
    ? value.surfaces
      .map((item) => normalizeHarnessApplyPreviewSurface(item))
      .filter((item): item is HarnessApplyPreviewSurface => item !== null)
    : [];
  return {
    snapshotBefore,
    expectedSnapshotId: typeof value.expected_snapshot_id === "string" ? value.expected_snapshot_id : null,
    changedSurfaceCount: typeof value.changed_surface_count === "number" ? value.changed_surface_count : 0,
    surfaces
  };
}


export function normalizeStringList(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const seen = new Set<string>();
  return value
    .filter((item): item is string => typeof item === "string")
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
    .filter((item) => {
      if (seen.has(item)) {
        return false;
      }
      seen.add(item);
      return true;
    });
}

export function normalizeUserMcpPermissions(value: unknown) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((item) => {
    if (!isRecord(item)) {
      return [];
    }
    const userId = typeof item.user_id === "string"
      ? item.user_id.trim()
      : typeof item.userId === "string"
        ? item.userId.trim()
        : "";
    const allowedTools = normalizeStringList(item.allowed_tools ?? item.allowedTools);
    const deniedTools = normalizeStringList(item.denied_tools ?? item.deniedTools);
    if (!userId || (!allowedTools.length && !deniedTools.length)) {
      return [];
    }
    return [{ userId, allowedTools, deniedTools }];
  });
}


function normalizeHitlDefaultAction(value: unknown): HitlDefaultAction {
  return value === "require_approval" || value === "reject" ? value : "auto";
}

function normalizeHitlRiskLevel(value: unknown): HitlRiskLevel {
  return value === "low" || value === "high" ? value : "medium";
}

export function normalizeHitlRules(value: unknown): HitlRule[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((item) => {
    if (!isRecord(item)) {
      return [];
    }
    const tool = typeof item.tool === "string" ? item.tool.trim() : "";
    const toolPrefix = typeof item.tool_prefix === "string"
      ? item.tool_prefix.trim()
      : typeof item.toolPrefix === "string"
        ? item.toolPrefix.trim()
        : "";
    if (!tool && !toolPrefix) {
      return [];
    }
    return [{
      tool: tool || null,
      toolPrefix: toolPrefix || null,
      requireApproval: Boolean(item.require_approval ?? item.requireApproval),
      riskLevel: normalizeHitlRiskLevel(item.risk_level ?? item.riskLevel)
    }];
  });
}

export function serializeHitlRules(rules: HitlRule[]) {
  return rules.map((rule) => ({
    tool: rule.tool?.trim() || null,
    tool_prefix: rule.toolPrefix?.trim() || null,
    require_approval: rule.requireApproval,
    risk_level: rule.riskLevel
  })).filter((rule) => rule.tool || rule.tool_prefix);
}

export function normalizeUserSkillPermissions(value: unknown) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.flatMap((item) => {
    if (!isRecord(item)) {
      return [];
    }
    const userId = typeof item.user_id === "string"
      ? item.user_id.trim()
      : typeof item.userId === "string"
        ? item.userId.trim()
        : "";
    const allowedSkills = normalizeStringList(item.allowed_skills ?? item.allowedSkills);
    const deniedSkills = normalizeStringList(item.denied_skills ?? item.deniedSkills);
    if (!userId || (!allowedSkills.length && !deniedSkills.length)) {
      return [];
    }
    return [{ userId, allowedSkills, deniedSkills }];
  });
}

export function normalizeSharedFrontendSettings(value: unknown): SharedFrontendSettings | null {
  if (!isRecord(value)) {
    return null;
  }
  return {
    brandTitle: typeof value.brand_title === "string" ? value.brand_title : "",
    brandSubtitle: typeof value.brand_subtitle === "string" ? value.brand_subtitle : "",
    mcpConfigPath: typeof value.mcp_config_path === "string" ? value.mcp_config_path : "",
    mcpBaseUrls: typeof value.mcp_base_urls === "string" ? value.mcp_base_urls : "",
    mcpDisabledUrls: Array.isArray(value.mcp_disabled_urls)
      ? value.mcp_disabled_urls.filter((item): item is string => typeof item === "string")
      : [],
    mcpLazyUrls: Array.isArray(value.mcp_lazy_urls)
      ? value.mcp_lazy_urls.filter((item): item is string => typeof item === "string")
      : [],
    mcpUserPermissions: normalizeUserMcpPermissions(value.mcp_user_permissions),
    skillUserPermissions: normalizeUserSkillPermissions(value.skill_user_permissions),
    agentPromptOverride: typeof value.agent_prompt_override === "string" ? value.agent_prompt_override : "",
    agentPromptAppend: typeof value.agent_prompt_append === "string" ? value.agent_prompt_append : "",
    modelId: typeof value.model_id === "string" ? value.model_id : "",
    temperature: typeof value.temperature === "string" ? value.temperature : "",
    maxTokens: typeof value.max_tokens === "string" ? value.max_tokens : "",
    maxIterations: typeof value.max_iterations === "string" ? value.max_iterations : "",
    topP: typeof value.top_p === "string" ? value.top_p : "",
    memoryMaintenanceSystemPrompt:
      typeof value.memory_maintenance_system_prompt === "string" ? value.memory_maintenance_system_prompt : "",
    memoryMaintenanceUserPrompt:
      typeof value.memory_maintenance_user_prompt === "string" ? value.memory_maintenance_user_prompt : "",
    skillLearningSystemPrompt:
      typeof value.skill_learning_system_prompt === "string" ? value.skill_learning_system_prompt : "",
    skillLearningUserPrompt:
      typeof value.skill_learning_user_prompt === "string" ? value.skill_learning_user_prompt : "",
    hitlEnabled: Boolean(value.hitl_enabled),
    hitlDefaultAction: normalizeHitlDefaultAction(value.hitl_default_action),
    hitlTimeoutSeconds: typeof value.hitl_timeout_seconds === "string"
      ? value.hitl_timeout_seconds
      : typeof value.hitl_timeout_seconds === "number"
        ? String(value.hitl_timeout_seconds)
        : "300",
    hitlRules: normalizeHitlRules(value.hitl_rules)
  };
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
  const userIdForPermissions = (settings.memoryUserId.trim() || config.userId || DEFAULT_MEMORY_USER_ID).trim();
  const mcpPermissions = settings.mcpUserPermissions.find((item) => item.userId === userIdForPermissions);
  if (mcpPermissions?.allowedTools.length) {
    formData.append("mcp_allowed_tools", JSON.stringify(mcpPermissions.allowedTools));
  }
  if (mcpPermissions?.deniedTools.length) {
    formData.append("mcp_denied_tools", JSON.stringify(mcpPermissions.deniedTools));
  }
  const skillPermissions = settings.skillUserPermissions.find((item) => item.userId === userIdForPermissions);
  if (skillPermissions?.allowedSkills.length) {
    formData.append("skill_allowed_names", JSON.stringify(skillPermissions.allowedSkills));
  }
  if (skillPermissions?.deniedSkills.length) {
    formData.append("skill_denied_names", JSON.stringify(skillPermissions.deniedSkills));
  }
  appendOptionalFormData(formData, "system_override", settings.agentPromptOverride);
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
  formData.append("hitl_enabled", settings.hitlEnabled ? "true" : "false");
  appendOptionalFormData(formData, "hitl_default_action", settings.hitlDefaultAction);
  appendOptionalFormData(formData, "hitl_timeout_seconds", settings.hitlTimeoutSeconds);
  if (settings.hitlRules.length) {
    formData.append("hitl_rules", JSON.stringify(serializeHitlRules(settings.hitlRules)));
  }
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
