import type { IconName } from "./icons";
import type {
  OutboundA2aTaskSummary,
  SessionCategory,
  SessionDeliveryOutcome,
  SessionRunStatus,
  SessionState,
  SessionSummary,
} from "./types";

export const SESSION_CATEGORIES: readonly SessionCategory[] = [
  "external",
  "scheduled",
  "spawned",
  "artifact",
];

export const KIND_NAMES: Readonly<Record<SessionCategory, string>> = {
  external: "From another app",
  scheduled: "Scheduled",
  spawned: "Started by an agent or you",
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

export function startedByOwner(run: Pick<SessionSummary, "category" | "spawner">): boolean {
  return run.category === "spawned" && run.spawner === null;
}

export function runKind(agent: string, run: Pick<SessionSummary, "category" | "spawner">): string {
  if (run.category !== "spawned") return KIND_NAMES[run.category];
  if (run.spawner === null) return "Started by you";
  return run.spawner === "main" ? `Started by ${agent}` : `Started by ${run.spawner}`;
}

export function isStoppableState(state: SessionState): boolean {
  return state === "forking" || state === "queued" || state === "running" || state === "idle";
}

export interface RunStatus {
  tone: "working" | "quiet" | "done" | "failed";
  text: string;
}

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

export function finishedOutcome(run: SessionSummary): string {
  const took =
    run.completed_at === null ? "" : ` after ${runDuration(run, Date.parse(run.started_at))}`;
  if (run.outcome === "failed") return run.error ? `Failed: ${run.error}` : "Failed";
  if (run.outcome === "cancelled") return `Stopped${took}`;
  if (run.interrupted) return "Cut short when Residuum stopped";
  return `Finished${took}`;
}

const ARTIFACT_SOURCE_PREFIX = "artifact:";

export function sessionArtifact(session: SessionSummary): string | null {
  if (session.category !== "artifact") return null;
  if (!session.source_label.startsWith(ARTIFACT_SOURCE_PREFIX)) return null;
  return session.source_label.slice(ARTIFACT_SOURCE_PREFIX.length) || null;
}

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

export function formatLocalDateTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : STARTED_FORMATTER.format(date);
}

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

export function outboundDuration(task: OutboundA2aTaskSummary, now: number): string {
  const start = Date.parse(task.started_at);
  return Number.isNaN(start) ? "" : formatDuration(now - start);
}

export function isTier(model: string): model is "small" | "medium" | "large" {
  return model === "small" || model === "medium" || model === "large";
}

export function runModel(run: Pick<SessionSummary, "model" | "resolved_model">): string | null {
  const tier = run.model !== null && isTier(run.model) ? run.model : null;
  if (run.resolved_model === null) return tier === null ? run.model : `The ${tier} tier`;
  return tier === null ? run.resolved_model : `${run.resolved_model}, from the ${tier} tier`;
}
