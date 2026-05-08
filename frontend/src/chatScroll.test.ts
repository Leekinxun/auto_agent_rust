import { describe, expect, it } from "vitest";

import {
  formatUnreadCount,
  getScrollButtonLabel,
  getViewportFollowDelta,
  hasScrollButtonUnreadAccent,
  isBottomWithinFollowThreshold,
  isNearScrollableBottom
} from "./chatScroll";

describe("chatScroll helpers", () => {
  it("formats unread counts with 99+ cap", () => {
    expect(formatUnreadCount(1)).toBe("1");
    expect(formatUnreadCount(9)).toBe("9");
    expect(formatUnreadCount(99)).toBe("99");
    expect(formatUnreadCount(120)).toBe("99+");
  });

  it("builds button labels from unread state", () => {
    expect(getScrollButtonLabel(false, 0)).toBe("回到底部");
    expect(getScrollButtonLabel(true, 0)).toBe("最新内容 ↓");
    expect(getScrollButtonLabel(false, 2)).toBe("最新消息 ↓");
    expect(getScrollButtonLabel(true, 5)).toBe("最新消息 ↓");
  });

  it("marks unread accent for new content or new turns", () => {
    expect(hasScrollButtonUnreadAccent(false, 0)).toBe(false);
    expect(hasScrollButtonUnreadAccent(true, 0)).toBe(true);
    expect(hasScrollButtonUnreadAccent(false, 1)).toBe(true);
  });

  it("detects when an inner scroll container is already near the bottom", () => {
    expect(isNearScrollableBottom(600, 280, 300)).toBe(true);
    expect(isNearScrollableBottom(600, 200, 300)).toBe(false);
    expect(isNearScrollableBottom(600, 200, 300, 100)).toBe(true);
  });

  it("computes viewport follow distance with a safety margin", () => {
    expect(getViewportFollowDelta(900, 860)).toBe(64);
    expect(getViewportFollowDelta(840, 860)).toBe(4);
    expect(getViewportFollowDelta(820, 860)).toBe(0);
  });

  it("detects whether the latest content bottom is already close to the visible bottom", () => {
    expect(isBottomWithinFollowThreshold(860, 850)).toBe(true);
    expect(isBottomWithinFollowThreshold(860, 790)).toBe(false);
    expect(isBottomWithinFollowThreshold(860, 810, 60)).toBe(true);
  });
});
