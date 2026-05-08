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

export function isNearScrollableBottom(scrollHeight: number, scrollTop: number, clientHeight: number, threshold = 24) {
  return Math.max(0, scrollHeight - (scrollTop + clientHeight)) <= threshold;
}

export function getViewportFollowDelta(targetBottom: number, visibleBottom: number, margin = 24) {
  return Math.max(0, targetBottom - (visibleBottom - margin));
}

export function isBottomWithinFollowThreshold(targetBottom: number, visibleBottom: number, threshold = 48) {
  return Math.abs(visibleBottom - targetBottom) <= threshold;
}
