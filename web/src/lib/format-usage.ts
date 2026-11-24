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
