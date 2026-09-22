// ── Plain-language labels for agent sessions ─────────────────────────

import type {
  SessionCategory,
  SessionDeliveryOutcome,
  SessionRunStatus,
  SessionState,
  SessionSummary,
} from "./types";

/** States in which a run is still live (listed from the registry). */
export function isLiveState(state: SessionState): boolean {
  return state !== "completed";
}

/** States in which a stop request can still take effect. */
export function isStoppableState(state: SessionState): boolean {
  return state === "forking" || state === "running" || state === "idle";
}

export function stateLabel(state: SessionState): string {
  switch (state) {
    case "forking":
      return "starting";
    case "running":
      return "working";
    case "idle":
      return "idle";
    case "completing":
      return "finishing";
    case "completed":
      return "finished";
  }
}

export function categoryDescription(category: SessionCategory): string {
  switch (category) {
    case "scheduled":
      return "Started on a schedule (a pulse or scheduled action)";
    case "external":
      return "Started by someone else or another system (a chat conversation or webhook)";
    case "spawned":
      return "Started by an agent";
  }
}

/** How a run ended, for a status line. */
export function runOutcomeText(status: SessionRunStatus, error: string | null): string {
  switch (status) {
    case "completed":
      return "Session finished.";
    case "cancelled":
      return "Session stopped.";
    case "failed":
      return error ? `Session failed: ${error}` : "Session failed.";
  }
}

/** Where a message sent from the sidebar landed, in plain language. */
export function deliveryOutcomeText(outcome: SessionDeliveryOutcome): string {
  switch (outcome) {
    case "live":
      return "Delivered.";
    case "resumed":
      return "This session had finished, so your message started a new run.";
    case "queued":
      return "This session is wrapping up. Your message will start a new run as soon as it finishes.";
  }
}

/** Compact duration: `45s`, `12m`, `3h 5m`, `2d 4h`. */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `${hours}h ${minutes % 60}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  return hours % 24 ? `${days}d ${hours % 24}h` : `${days}d`;
}

/** How long a run has been going (live) or took (finished). */
export function runDuration(session: SessionSummary, now: number): string {
  const start = Date.parse(session.started_at);
  if (Number.isNaN(start)) return "";
  const end = session.completed_at ? Date.parse(session.completed_at) : now;
  return formatDuration((Number.isNaN(end) ? now : end) - start);
}

const STARTED_FORMATTER = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

/** When a run started, as a short local date and time. */
export function formatStarted(session: SessionSummary): string {
  const start = new Date(session.started_at);
  return Number.isNaN(start.getTime()) ? session.started_at : STARTED_FORMATTER.format(start);
}
