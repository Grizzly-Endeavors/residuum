// ── Plain-language words for agent sessions and outbound tasks ───────

import type { IconName } from "./icons";
import type {
  OutboundA2aTaskSummary,
  SessionCategory,
  SessionDeliveryOutcome,
  SessionRunStatus,
  SessionState,
  SessionSummary,
} from "./types";

/** Every session category, in the order Activity's kind filter lists them. */
export const SESSION_CATEGORIES: readonly SessionCategory[] = [
  "external",
  "scheduled",
  "spawned",
  "artifact",
];

/** Each kind of run, as the Finished filter and a run's details name it. */
export const KIND_NAMES: Readonly<Record<SessionCategory, string>> = {
  external: "From another app",
  scheduled: "Scheduled",
  spawned: "Started by an agent",
  artifact: "From a workbench page",
};

const KIND_ICONS: Readonly<Record<SessionCategory, IconName>> = {
  external: "hash",
  scheduled: "clock",
  spawned: "layers",
  artifact: "page",
};

export function runIcon(category: SessionCategory): IconName {
  return KIND_ICONS[category];
}

/** How a run started, in plain words. A spawner of `main` is the agent's own conversation. */
export function runKind(agent: string, run: Pick<SessionSummary, "category" | "spawner">): string {
  if (run.category !== "spawned") return KIND_NAMES[run.category];
  return run.spawner === null || run.spawner === "main"
    ? `Started by ${agent}`
    : `Started by ${run.spawner}`;
}

/** States in which a stop request can still take effect. */
export function isStoppableState(state: SessionState): boolean {
  return state === "forking" || state === "queued" || state === "running" || state === "idle";
}

/** How a run or task is doing, for its status mark: a tone and a few words. */
export interface RunStatus {
  tone: "working" | "quiet" | "done" | "failed";
  text: string;
}

/**
 * What a status reads from a run: a full summary, or a live run as the
 * overview lists it, which has no outcome because it hasn't ended.
 */
export type RunStatusSource = Pick<SessionSummary, "state" | "started_at"> &
  Partial<Pick<SessionSummary, "completed_at" | "outcome" | "interrupted">>;

export function runStatus(run: RunStatusSource, now: number): RunStatus {
  switch (run.state) {
    case "forking":
      return { tone: "working", text: "Starting" };
    case "queued":
      return { tone: "working", text: "Queued" };
    case "running":
      return { tone: "working", text: `Working, ${runDuration(run, now)}` };
    case "idle":
      return { tone: "quiet", text: `Idle, ${runDuration(run, now)}` };
    case "completing":
      return { tone: "quiet", text: "Finishing" };
    case "completed":
      break;
  }
  if (run.outcome === "failed") return { tone: "failed", text: "Failed" };
  if (run.outcome === "cancelled") return { tone: "quiet", text: "Stopped" };
  if (run.interrupted) return { tone: "quiet", text: "Interrupted" };
  return { tone: "done", text: "Finished" };
}

/** How a finished run ended, as its row says it. */
export function finishedOutcome(run: SessionSummary): string {
  const took =
    run.completed_at === null ? "" : ` after ${runDuration(run, Date.parse(run.started_at))}`;
  if (run.outcome === "failed") return run.error ? `Failed: ${run.error}` : "Failed";
  if (run.outcome === "cancelled") return `Stopped${took}`;
  if (run.interrupted) return "Cut short when Residuum stopped";
  return `Finished${took}`;
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

/** Where a message sent to a session landed, in plain language. */
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
export function runDuration(
  session: Pick<RunStatusSource, "started_at" | "completed_at">,
  now: number,
): string {
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

/** An RFC 3339 timestamp as a short local date and time. */
export function formatLocalDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : STARTED_FORMATTER.format(date);
}

/** Where a task sent to another agent stands, for its status mark. */
export function outboundStatus(task: OutboundA2aTaskSummary, now: number): RunStatus {
  if (task.unreachable_since) {
    const since = Date.parse(task.unreachable_since);
    const gone = Number.isNaN(since) ? "" : ` for ${formatDuration(now - since)}`;
    return { tone: "failed", text: `Can't reach ${task.agent}${gone}` };
  }
  switch (task.state) {
    case "submitted":
      return { tone: "working", text: "Sent" };
    case "working":
      return { tone: "working", text: "Working" };
    case "input_required":
      return { tone: "quiet", text: "Waiting on your agent's reply" };
    case "auth_required":
      return { tone: "quiet", text: "Waiting on sign-in" };
    default: {
      const words = task.state.replace(/_/g, " ");
      return { tone: "quiet", text: words.charAt(0).toUpperCase() + words.slice(1) };
    }
  }
}

/** How long ago a task was sent to another agent. */
export function outboundDuration(task: OutboundA2aTaskSummary, now: number): string {
  const start = Date.parse(task.started_at);
  return Number.isNaN(start) ? "" : formatDuration(now - start);
}
