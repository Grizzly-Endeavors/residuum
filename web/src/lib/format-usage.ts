// ── Elapsed time / token count formatting ────────────────────────────
//
// Shared by the activity line, the session panel and the conversation size.
// Kept deliberately quiet: short, muted strings, never a raw number dump.

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

/**
 * A token count as words, for figures read in plain language: about three
 * words to every four tokens, rounded to what a reader takes in at a glance
 * (`"740 words"`, `"14,000 words"`, `"1.2 million words"`).
 */
export function formatApproxWords(tokens: number): string {
  const words = Math.max(0, tokens) * 0.75;
  if (words >= 1_000_000) {
    return `${trimTrailingZero((words / 1_000_000).toFixed(1))} million words`;
  }
  if (words < 1) return "no words";
  const step = words < 1_000 ? 10 : 10 ** (Math.floor(Math.log10(words)) - 1);
  const rounded = Math.max(step, Math.round(words / step) * step);
  return `${rounded.toLocaleString("en-US")} words`;
}

function trimTrailingZero(value: string): string {
  return value.endsWith(".0") ? value.slice(0, -2) : value;
}
