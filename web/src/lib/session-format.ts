// ── Plain-language labels for agent sessions ─────────────────────────

import type {
  SessionCategory,
  SessionDeliveryOutcome,
  SessionRunStatus,
  SessionState,
  SessionSummary,
} from "./types";

/** Every session category, in the order the sidebar groups them. */
export const SESSION_CATEGORIES: readonly SessionCategory[] = [
  "external",
  "scheduled",
  "spawned",
  "artifact",
];

/** Sessions split into their categories' sidebar groups, each keeping the list's order. */
export function groupByCategory(
  sessions: readonly SessionSummary[],
): Record<SessionCategory, SessionSummary[]> {
  const groups = Object.fromEntries(
    SESSION_CATEGORIES.map((category) => [category, [] as SessionSummary[]]),
  ) as Record<SessionCategory, SessionSummary[]>;
  for (const session of sessions) groups[session.category].push(session);
  return groups;
}

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
    case "artifact":
      return "Started by a workbench artifact";
  }
}

/** A category's name as a sidebar group heading. */
export function categoryHeading(category: SessionCategory): string {
  switch (category) {
    case "scheduled":
      return "Scheduled";
    case "external":
      return "External";
    case "spawned":
      return "Spawned";
    case "artifact":
      return "Artifacts";
  }
}

/** What a sidebar group says when none of its sessions are running. */
export function categoryIdleText(category: SessionCategory): string {
  switch (category) {
    case "scheduled":
      return "Nothing running. Pulses and scheduled actions show up here while they run.";
    case "external":
      return "Nothing running. Conversations with other people and webhook calls show up here while they run.";
    case "spawned":
      return "Nothing running. Work your agent hands off shows up here while it runs.";
    case "artifact":
      return "Nothing running. Work a workbench artifact starts shows up here while it runs.";
  }
}

/** Prefix of an artifact session's source label (`artifact:<name>`). */
const ARTIFACT_SOURCE_PREFIX = "artifact:";

/**
 * The workbench artifact that started a session, from its source label, or
 * `null` for a session no artifact started.
 */
export function sessionArtifact(session: SessionSummary): string | null {
  if (session.category !== "artifact") return null;
  if (!session.source_label.startsWith(ARTIFACT_SOURCE_PREFIX)) return null;
  return session.source_label.slice(ARTIFACT_SOURCE_PREFIX.length) || null;
}

/** What started a session, for a row or header: the artifact's name, or the source label. */
export function sessionSourceText(session: SessionSummary): string {
  return sessionArtifact(session) ?? session.source_label;
}

/**
 * The sessions an artifact started, in the order they appear in `sessions`,
 * for its activity panel. Kept current by whatever keeps `sessions` current
 * (session frames), so the panel needs no fetch of its own.
 */
export function sessionsStartedByArtifact(
  sessions: readonly SessionSummary[],
  artifact: string,
): SessionSummary[] {
  return sessions.filter((s) => sessionArtifact(s) === artifact);
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
