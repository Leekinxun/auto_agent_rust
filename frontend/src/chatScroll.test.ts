import { describe, expect, it } from "vitest";

import { formatUnreadCount, getScrollButtonLabel, hasScrollButtonUnreadAccent } from "./chatScroll";

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
});
