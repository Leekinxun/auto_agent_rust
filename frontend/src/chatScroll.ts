export function formatUnreadCount(count: number) {
  if (count > 99) {
    return "99+";
  }
  return String(count);
}

export function getScrollButtonLabel(hasUnreadUpdates: boolean, unreadTurnCount: number) {
  if (unreadTurnCount > 0) {
    return "最新消息 ↓";
  }
  if (hasUnreadUpdates) {
    return "最新内容 ↓";
  }
  return "回到底部";
}

export function hasScrollButtonUnreadAccent(hasUnreadUpdates: boolean, unreadTurnCount: number) {
  return hasUnreadUpdates || unreadTurnCount > 0;
}
