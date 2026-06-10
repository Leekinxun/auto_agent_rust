import { describe, expect, it } from "vitest";

import {
  CHAT_MODES,
  buildMcpPreviewQueryParams,
  buildFormData,
  collectDownloadFiles,
  createInitialChats,
  getPathForView,
  getViewFromPath,
  isChatView,
  normalizeHarnessApplyPreview,
  normalizeHarnessApprovals,
  normalizeHarnessDecisions,
  normalizeHarnessDrafts,
  normalizeHarnessSignals,
  normalizeHarnessSnapshot,
  normalizeHarnessTraces,
  normalizeSharedFrontendSettings,
  normalizeMcpPreviewServers,
  parseMcpBaseUrlsInput,
  parseThinking,
  stripThinkingContent
} from "./utils";

describe("frontend chat surface", () => {
  it("only exposes the two streaming chat modes", () => {
    expect(Object.keys(CHAT_MODES)).toEqual(["stream", "memoryStream"]);
    expect(Object.keys(createInitialChats())).toEqual(["stream", "memoryStream"]);
    expect(createInitialChats().stream.queue).toEqual([]);
    expect(createInitialChats().stream.stopRequested).toBe(false);
    expect(createInitialChats().stream.steeringPending).toBe(false);
    expect(createInitialChats().stream.steeringPreview).toBe("");
  });

  it("keeps active view paths for the two streaming pages", () => {
    expect(getPathForView("stream")).toBe("/chat/stream");
    expect(getPathForView("memoryStream")).toBe("/chat/memory-stream");
    expect(getPathForView("harness")).toBe("/harness");
  });

  it("maps legacy sync routes onto the current streaming views", () => {
    expect(getViewFromPath("/")).toBe("stream");
    expect(getViewFromPath("/chat/stream")).toBe("stream");
    expect(getViewFromPath("/chat/normal")).toBe("stream");
    expect(getViewFromPath("/chat/memory-run")).toBe("memoryStream");
    expect(getViewFromPath("/chat/memory-stream")).toBe("memoryStream");
    expect(getViewFromPath("/harness")).toBe("harness");
  });

  it("distinguishes chat views from management views", () => {
    expect(isChatView("stream")).toBe(true);
    expect(isChatView("memoryStream")).toBe(true);
    expect(isChatView("harness")).toBe(false);
    expect(isChatView("skills")).toBe(false);
    expect(isChatView("settings")).toBe(false);
  });

  it("includes max iterations in chat form data when configured", () => {
    const formData = buildFormData(
      CHAT_MODES.stream,
      [],
      "hello",
      [],
      {
        apiBase: "http://localhost:8080",
        brandTitle: "brand",
        brandSubtitle: "subtitle",
        memoryUserId: "user-1",
        mcpConfigPath: "/tmp/mcp.json",
        mcpBaseUrls: "http://mcp-a.example/mcp\nhttp://mcp-b.example/mcp",
        mcpDisabledUrls: ["http://mcp-b.example/mcp"],
        mcpLazyUrls: ["http://mcp-a.example/mcp"],
        mcpUserPermissions: [{ userId: "user-1", allowedTools: ["read_file"], deniedTools: ["delete_file"] }],
        skillUserPermissions: [{ userId: "user-1", allowedSkills: ["read_document"], deniedSkills: ["get_oil_data"] }],
        agentPromptOverride: "base override",
        agentPromptAppend: "extra prompt",
        modelId: "demo-model",
        temperature: "0.2",
        maxTokens: "4096",
        maxIterations: "9",
        topP: "0.9",
        memoryMaintenanceSystemPrompt: "memory sys",
        memoryMaintenanceUserPrompt: "memory user",
        skillLearningSystemPrompt: "skill sys",
        skillLearningUserPrompt: "skill user",
        hitlEnabled: true,
        hitlDefaultAction: "auto",
        hitlTimeoutSeconds: "300",
        hitlRules: [{ tool: "write_file", toolPrefix: null, requireApproval: true, riskLevel: "high" }]
      }
    );

    expect(formData.get("max_iterations")).toBe("9");
    expect(formData.get("mcp_config_path")).toBe("/tmp/mcp.json");
    expect(formData.get("mcp_base_urls")).toBe("[\"http://mcp-a.example/mcp\",\"http://mcp-b.example/mcp\"]");
    expect(formData.get("mcp_disabled_urls")).toBe("[\"http://mcp-b.example/mcp\"]");
    expect(formData.get("mcp_lazy_urls")).toBe("[\"http://mcp-a.example/mcp\"]");
    expect(formData.get("mcp_allowed_tools")).toBe("[\"read_file\"]");
    expect(formData.get("mcp_denied_tools")).toBe("[\"delete_file\"]");
    expect(formData.get("skill_allowed_names")).toBe("[\"read_document\"]");
    expect(formData.get("skill_denied_names")).toBe("[\"get_oil_data\"]");
    expect(formData.get("system_override")).toBe("base override");
    expect(formData.get("system_append")).toBe("extra prompt");
    expect(formData.get("memory_maintenance_system")).toBe("memory sys");
    expect(formData.get("skill_learning_user_template")).toBe("skill user");
  });

  it("hides incomplete think blocks from visible streaming text", () => {
    expect(stripThinkingContent("<think>正在思考")).toBe("");
    expect(stripThinkingContent("已输出<think>正在思考")).toBe("已输出");
    expect(stripThinkingContent("前文<think>思考</think>后文")).toBe("前文后文");
  });

  it("parses completed think blocks and drops unfinished trailing think content", () => {
    expect(parseThinking("前文<think>第一步</think>后文<think>未完成")).toEqual([
      { type: "text", content: "前文" },
      { type: "thinking", content: "第一步" },
      { type: "text", content: "后文" }
    ]);
  });

  it("only shows downloads explicitly returned by the backend", () => {
    expect(
      collectDownloadFiles({
        id: "assistant-1",
        role: "assistant",
        text: "已更新 USER.md，并参考 /app/outputs/report.md 继续处理。",
        attachments: [],
        processing: false,
        outputFiles: [{ name: "report.md", path: "/app/outputs/report.md" }],
        processItems: []
      })
    ).toEqual([{ name: "report.md", path: "/app/outputs/report.md" }]);
  });

  it("deduplicates and parses MCP base urls from textarea input", () => {
    expect(parseMcpBaseUrlsInput("http://a/mcp\nhttp://b/mcp, http://a/mcp")).toEqual([
      "http://a/mcp",
      "http://b/mcp"
    ]);
  });

  it("builds unfiltered MCP catalog preview query params for settings selectors", () => {
    const params = buildMcpPreviewQueryParams({
      configPath: " /tmp/mcp.json ",
      rawBaseUrls: "http://a/mcp\nhttp://b/mcp, http://a/mcp",
      disabledUrls: ["http://b/mcp/"],
      lazyUrls: ["http://a/mcp/", "http://b/mcp/"],
      userId: "user-1",
      userPermissions: [{ userId: "user-1", allowedTools: ["mcp_a__tool_a"], deniedTools: ["mcp_b__tool_b"] }],
      ignoreUserPermissions: true
    });

    expect(params.get("config_path")).toBe("/tmp/mcp.json");
    expect(params.get("base_urls")).toBe('["http://a/mcp","http://b/mcp"]');
    expect(params.get("disabled_urls")).toBe('["http://b/mcp"]');
    expect(params.get("lazy_urls")).toBe('["http://a/mcp"]');
    expect(params.get("user_id")).toBe("user-1");
    expect(params.get("ignore_user_permissions")).toBe("true");
    expect(params.has("allowed_tools")).toBe(false);
    expect(params.has("denied_tools")).toBe(false);
  });

  it("builds permission-filtered MCP preview query params for effective chat checks", () => {
    const params = buildMcpPreviewQueryParams({
      configPath: "",
      rawBaseUrls: "http://a/mcp",
      userId: "user-1",
      userPermissions: [{ userId: "user-1", allowedTools: ["mcp_a__tool_a"], deniedTools: ["mcp_a__tool_b"] }]
    });

    expect(params.get("base_urls")).toBe('["http://a/mcp"]');
    expect(params.get("allowed_tools")).toBe('["mcp_a__tool_a"]');
    expect(params.get("denied_tools")).toBe('["mcp_a__tool_b"]');
    expect(params.has("ignore_user_permissions")).toBe(false);
  });

  it("normalizes MCP preview servers from snake_case payloads", () => {
    expect(normalizeMcpPreviewServers([
      {
        endpoint: "http://a/mcp",
        endpoint_key: "demo_key",
        mode: "lazy",
        ok: true,
        tool_count: 2,
        tools: [
          { name: "tool_a", description: "desc a" },
          { name: "tool_b" }
        ]
      }
    ])).toEqual([
      {
        endpoint: "http://a/mcp",
        endpointKey: "demo_key",
        mode: "lazy",
        ok: true,
        toolCount: 2,
        tools: [
          { name: "tool_a", description: "desc a" },
          { name: "tool_b", description: "" }
        ],
        error: undefined
      }
    ]);
  });

  it("keeps disabled MCP servers visible in preview payloads", () => {
    expect(normalizeMcpPreviewServers([
      {
        endpoint: "http://disabled/mcp",
        mode: "disabled",
        ok: true,
        tool_count: 1,
        tools: [{ name: "hidden_tool", description: "still previewable" }]
      }
    ])).toEqual([
      {
        endpoint: "http://disabled/mcp",
        endpointKey: "http://disabled/mcp",
        mode: "disabled",
        ok: true,
        toolCount: 1,
        tools: [{ name: "hidden_tool", description: "still previewable" }],
        error: undefined
      }
    ]);
  });

  it("normalizes harness observability payloads from snake_case responses", () => {
    expect(normalizeHarnessSnapshot({
      snapshot_id: "hsnap-1",
      generated_at_ms: 1000,
      memory_only_self_evolution: true,
      surfaces: [
        {
          key: "system.base",
          source: { kind: "file", path: "harness/system/base.md" },
          sha1: "abc",
          bytes: 3,
          content: "xyz"
        }
      ]
    })).toEqual({
      snapshotId: "hsnap-1",
      generatedAtMs: 1000,
      memoryOnlySelfEvolution: true,
      surfaces: [
        {
          key: "system.base",
          source: { kind: "file", path: "harness/system/base.md" },
          sha1: "abc",
          bytes: 3,
          content: "xyz"
        }
      ]
    });

    expect(normalizeHarnessSignals({
      inspected_traces: 2,
      memory_traces: 1,
      stateless_traces: 1,
      success_traces: 1,
      error_traces: 1,
      final_reply_recovered_traces: 1,
      max_iterations_traces: 1,
      self_evolution_executed_traces: 1,
      avg_iterations: 3.5,
      avg_tool_calls: 4.5,
      top_tools: [{ name: "read_file", count: 4 }],
      candidate_signals: [{ key: "tool_churn", severity: "medium", summary: "too many tools", trace_ids: ["t1"] }],
      recent_trace_ids: ["t1", "t0"],
      memory_only_self_evolution: true
    })?.candidateSignals[0].traceIds).toEqual(["t1"]);

    expect(normalizeHarnessTraces([{
      trace_id: "trace-1",
      harness_snapshot_id: "hsnap-1",
      started_at_ms: 1,
      finished_at_ms: 2,
      run_kind: "sync",
      mode: "memory",
      request: {
        session_id: "s1",
        user_id_present: true,
        history_items: 2,
        uploaded_files: 0,
        memory_snapshot_injected: true,
        self_evolution_allowed: true,
        resolved_model_id: "demo",
        resolved_max_iterations: 8,
        temperature: 0.2,
        top_p: 0.9,
        mcp_base_urls: 1,
        mcp_disabled_urls: 0,
        mcp_lazy_urls: 0
      },
      prompts: {
        top_level_system: { kind: "builtin" },
        system_append: { kind: "none" },
        final_answer_recovery: { kind: "file", path: "harness/middleware/final-answer-recovery.md" },
        subagent_shared: { kind: "builtin" },
        subagent_explore: { kind: "builtin" },
        subagent_general: { kind: "builtin" },
        memory_maintenance_system: { kind: "builtin" },
        memory_maintenance_user_template: { kind: "builtin" },
        skill_learning_system: { kind: "builtin" },
        skill_learning_user_template: { kind: "builtin" }
      },
      outcome: {
        status: "success",
        finish_reason: "stop",
        error: null,
        iterations: 2,
        tool_calls: 1,
        tool_names: ["read_file"],
        reply_chars: 42,
        output_files: 0,
        output_file_names: [],
        used_skill_names: [],
        skills_updated: 0,
        final_reply_recovered: false,
        self_evolution_executed: true
      }
    }])[0].harnessSnapshotId).toBe("hsnap-1");

    expect(normalizeHarnessDecisions([{
      decision_id: "d1",
      created_at_ms: 1000,
      title: "decision",
      summary: "summary",
      rationale: "why",
      expected_impact: ["impact"],
      changed_surfaces: ["system.base"],
      validation_plan: ["verify"],
      mode_scope: "memory_only",
      status: "proposed",
      related_trace_ids: ["t1"],
      snapshot_before_id: "hsnap-1",
      snapshot_after_id: null
    }])[0].decisionId).toBe("d1");

    expect(normalizeHarnessDrafts([{
      draft_id: "draft-1",
      signal_key: "tool_churn",
      severity: "medium",
      title: "draft",
      summary: "summary",
      rationale: "why",
      expected_impact: ["impact"],
      changed_surfaces: ["tools.descriptions"],
      validation_plan: ["verify"],
      mode_scope: "memory_only",
      recommended_status: "proposed",
      related_trace_ids: ["t1"],
      snapshot_before_id: "hsnap-1"
    }])[0].draftId).toBe("draft-1");

    expect(normalizeHarnessApprovals([{
      approval_id: "approval-1",
      created_at_ms: 2000,
      decision_id: "decision-1",
      title: "approval",
      summary: "approved summary",
      approved_by: "alice",
      approval_note: "looks good",
      mode_scope: "memory_only",
      related_trace_ids: ["t1"],
      snapshot_before_id: "hsnap-1",
      snapshot_after_id: "hsnap-2",
      changed_surfaces: [{
        surface_key: "system.base",
        path: "harness/system/base.md",
        changed: true,
        before_sha1: "a",
        after_sha1: "b",
        before_bytes: 1,
        after_bytes: 2,
        byte_delta: 1,
        before_lines: 1,
        after_lines: 2,
        line_delta: 1,
        before_content: "old",
        after_content: "new"
      }],
      runtime_reloaded: true,
      status: "reverted",
      reverted_from_approval_id: "approval-0"
    }])[0].revertedFromApprovalId).toBe("approval-0");

    expect(normalizeHarnessApplyPreview({
      snapshot_before: {
        snapshot_id: "hsnap-1",
        generated_at_ms: 1000,
        memory_only_self_evolution: true,
        surfaces: []
      },
      expected_snapshot_id: "hsnap-1",
      changed_surface_count: 1,
      surfaces: [{
        surface_key: "system.base",
        path: "harness/system/base.md",
        changed: true,
        before_sha1: "a",
        after_sha1: "b",
        before_bytes: 1,
        after_bytes: 2,
        byte_delta: 1,
        before_lines: 1,
        after_lines: 2,
        line_delta: 1,
        before_content: "old",
        after_content: "new"
      }]
    })?.surfaces[0].afterContent).toBe("new");

    expect(normalizeSharedFrontendSettings({
      brand_title: "共享标题",
      brand_subtitle: "共享副标题",
      mcp_config_path: "config/mcp.json",
      mcp_base_urls: "http://demo/mcp",
      mcp_disabled_urls: ["http://a"],
      mcp_lazy_urls: ["http://b"],
      mcp_user_permissions: [{ user_id: "user-1", allowed_tools: ["read_file"], denied_tools: ["delete_file"] }],
      skill_user_permissions: [{ user_id: "user-1", allowed_skills: ["read_document"], denied_skills: ["get_oil_data"] }],
      agent_prompt_append: "append",
      model_id: "demo-model",
      temperature: "0.2",
      max_tokens: "4096",
      max_iterations: "12",
      top_p: "0.9",
      memory_maintenance_system_prompt: "sys",
      memory_maintenance_user_prompt: "user",
      skill_learning_system_prompt: "skill sys",
      skill_learning_user_prompt: "skill user"
    })?.mcpUserPermissions[0].allowedTools).toEqual(["read_file"]);
  });
});
