// ── Elapsed time / token count formatting ────────────────────────────
//
// Shared by the running-turn indicator and the chat footer. Kept
// deliberately quiet: short, muted strings, never a raw number dump.

/**
 * Format a duration in milliseconds the way a status line does:
 * `"45s"`, `"1m 12s"`, `"1h 03m"`. Anything under a second shows as `"0s"`.
 */
export function formatElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  if (hours > 0) {
    return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  }
  if (minutes > 0) {
    return `${minutes}m ${String(seconds).padStart(2, "0")}s`;
  }
  return `${seconds}s`;
}

/**
 * Format a token count the way a status line does: `"4.3k"`, `"1.2M"`,
 * or the plain number under 1000. One decimal place, trimmed when it
 * would just be `.0`.
 */
export function formatTokenCount(count: number): string {
  const abs = Math.abs(count);
  if (abs >= 1_000_000) {
    return `${trimTrailingZero((count / 1_000_000).toFixed(1))}M`;
  }
  if (abs >= 1_000) {
    return `${trimTrailingZero((count / 1_000).toFixed(1))}k`;
  }
  return String(count);
}

function trimTrailingZero(value: string): string {
  return value.endsWith(".0") ? value.slice(0, -2) : value;
}
