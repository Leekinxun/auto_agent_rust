import { describe, expect, it } from "vitest";

import { CHAT_MODES, createInitialChats, getPathForView, getViewFromPath, isChatView } from "./utils";

describe("frontend chat surface", () => {
  it("only exposes the two streaming chat modes", () => {
    expect(Object.keys(CHAT_MODES)).toEqual(["stream", "memoryStream"]);
    expect(Object.keys(createInitialChats())).toEqual(["stream", "memoryStream"]);
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
});
