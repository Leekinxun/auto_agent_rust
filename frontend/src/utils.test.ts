import { describe, expect, it } from "vitest";

import { CHAT_MODES, buildFormData, createInitialChats, getPathForView, getViewFromPath, isChatView, parseThinking, stripThinkingContent } from "./utils";

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
});
