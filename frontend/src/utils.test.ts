import { describe, expect, it } from "vitest";

import {
  CHAT_MODES,
  buildFormData,
  collectDownloadFiles,
  createInitialChats,
  getPathForView,
  getViewFromPath,
  isChatView,
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
  });

  it("keeps active view paths for the two streaming pages", () => {
    expect(getPathForView("stream")).toBe("/chat/stream");
    expect(getPathForView("memoryStream")).toBe("/chat/memory-stream");
  });

  it("maps legacy sync routes onto the current streaming views", () => {
    expect(getViewFromPath("/")).toBe("stream");
    expect(getViewFromPath("/chat/stream")).toBe("stream");
    expect(getViewFromPath("/chat/normal")).toBe("stream");
    expect(getViewFromPath("/chat/memory-run")).toBe("memoryStream");
    expect(getViewFromPath("/chat/memory-stream")).toBe("memoryStream");
  });

  it("distinguishes chat views from management views", () => {
    expect(isChatView("stream")).toBe(true);
    expect(isChatView("memoryStream")).toBe(true);
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
        agentPromptAppend: "extra prompt",
        modelId: "demo-model",
        temperature: "0.2",
        maxTokens: "4096",
        maxIterations: "9",
        topP: "0.9",
        memoryMaintenanceSystemPrompt: "memory sys",
        memoryMaintenanceUserPrompt: "memory user",
        skillLearningSystemPrompt: "skill sys",
        skillLearningUserPrompt: "skill user"
      }
    );

    expect(formData.get("max_iterations")).toBe("9");
    expect(formData.get("mcp_config_path")).toBe("/tmp/mcp.json");
    expect(formData.get("mcp_base_urls")).toBe("[\"http://mcp-a.example/mcp\",\"http://mcp-b.example/mcp\"]");
    expect(formData.get("mcp_disabled_urls")).toBe("[\"http://mcp-b.example/mcp\"]");
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

  it("normalizes MCP preview servers from snake_case payloads", () => {
    expect(normalizeMcpPreviewServers([
      {
        endpoint: "http://a/mcp",
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
});
