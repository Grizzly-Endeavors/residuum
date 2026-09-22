// ── Relative time ────────────────────────────────────────────────────

/** How long ago `then` was, compactly: "just now", "5m ago", "3h ago", "2d ago". */
export function relativeTime(then: Date | string, now: number = Date.now()): string {
  const at = typeof then === "string" ? new Date(then) : then;
  const seconds = Math.max(0, Math.round((now - at.getTime()) / 1000));
  if (Number.isNaN(seconds)) return "";
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}
