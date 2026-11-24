import { SvelteMap, SvelteSet } from "svelte/reactivity";
import { ApiError, fetchOutboundA2aTasks, fetchSessions, stopSession } from "./api";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";
import { requireAgent } from "./paths";
import { SESSION_CATEGORIES } from "./session-format";
import { isoNow } from "./time";
import type {
  OutboundA2aTaskSummary,
  ServerMessage,
  SessionCategory,
  SessionSummary,
} from "./types";

export type SessionFrame = Extract<ServerMessage, { type: `session_${string}` }>;

export type RunFrame = Extract<SessionFrame, { run_id: string }>;

export function isSessionFrame(msg: ServerMessage): msg is SessionFrame {
  return msg.type.startsWith("session_");
}

export interface SessionStartFailure {
  error: string;
  details: string | null;
}

export type FinishedKind = SessionCategory | "all";

const PAGE_SIZE = 25;

const REFRESH_DEBOUNCE_MS = 250;

export class CompletedRuns {
  runs = $state<SessionSummary[]>([]);
  nextCursor = $state<string | null>(null);
  loaded = $state(false);
  error = $state<string | null>(null);
  loadingMore = $state(false);

  constructor(
    readonly category: SessionCategory | null,
    private readonly agent: string | null,
  ) {}

  private query(before?: string): Parameters<typeof fetchSessions>[1] {
    return { category: this.category ?? undefined, before, limit: PAGE_SIZE };
  }

  async loadFirst(): Promise<void> {
    try {
      const page = await fetchSessions(requireAgent(this.agent), this.query());
      this.mergeFirstPage(page.completed, page.next_cursor);
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't load the finished runs." });
    }
  }

  async loadMore(): Promise<void> {
    const cursor = this.nextCursor;
    if (!cursor || this.loadingMore) return;
    this.loadingMore = true;
    try {
      const page = await fetchSessions(requireAgent(this.agent), this.query(cursor));
      const known = this.runs.map((s) => s.run_id);
      this.runs.push(...page.completed.filter((s) => !known.includes(s.run_id)));
      this.nextCursor = page.next_cursor;
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, { action: "Couldn't load older finished runs." }),
      );
    } finally {
      this.loadingMore = false;
    }
  }

  prepend(run: SessionSummary): void {
    this.runs = [run, ...this.runs.filter((s) => s.run_id !== run.run_id)];
  }

  mergeFirstPage(first: SessionSummary[], firstCursor: string | null): void {
    const oldCursor = this.nextCursor;
    const last = first[first.length - 1];
    this.loaded = true;
    this.error = null;
    if (firstCursor === null || !last || this.runs.length <= first.length) {
      this.runs = first;
      this.nextCursor = firstCursor;
      return;
    }
    const inFirst = first.map((s) => s.run_id);
    const tail = this.runs.filter(
      (s) => !inFirst.includes(s.run_id) && compareRunsNewestFirst(s, last) > 0,
    );
    this.runs = [...first, ...tail];
    this.nextCursor = tail.length ? oldCursor : firstCursor;
  }
}

export interface SessionsStoreDeps {
  agent: string | null;
}

export async function lookUpNewestRun(
  agent: string,
  address: string,
  noRecord?: { message: string; details?: string },
): Promise<string | null> {
  try {
    const page = await fetchSessions(agent, { address, limit: 1 });
    const run = page.live[0] ?? page.completed[0];
    if (run) return run.run_id;
    if (noRecord) {
      notifications.surface("error", noRecord.message, noRecord.details);
    } else {
      notifications.surface("error", `There's no record of the session ${address}.`);
    }
  } catch (err) {
    notifications.surface(
      "error",
      userErrorMessage(err, { action: `Couldn't open the session ${address}.` }),
    );
  }
  return null;
}

export class SessionsStore {
  live = $state<SessionSummary[]>([]);
  readonly finished: Record<FinishedKind, CompletedRuns>;
  finishedKind = $state<FinishedKind>("all");
  loaded = $state(false);
  listError = $state<string | null>(null);
  errors = new SvelteMap<string, string>();
  stopping = new SvelteSet<string>();
  private readonly startFailures = new SvelteMap<string, SessionStartFailure>();
  outbound = $state<OutboundA2aTaskSummary[]>([]);
  outboundError = $state<string | null>(null);

  private refreshing = false;
  private refreshAgain = false;
  private refreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly deps: SessionsStoreDeps) {
    this.finished = {
      all: new CompletedRuns(null, deps.agent),
      external: new CompletedRuns("external", deps.agent),
      scheduled: new CompletedRuns("scheduled", deps.agent),
      spawned: new CompletedRuns("spawned", deps.agent),
      artifact: new CompletedRuns("artifact", deps.agent),
    };
  }

  get agent(): string | null {
    return this.deps.agent;
  }

  get runningCount(): number {
    return this.live.length + this.outbound.length;
  }

  async refresh(): Promise<void> {
    void this.refreshOutbound();
    if (this.refreshing) {
      this.refreshAgain = true;
      return;
    }
    this.refreshing = true;
    try {
      const page = await fetchSessions(requireAgent(this.deps.agent), { limit: PAGE_SIZE });
      this.live = [...page.live].sort(compareRunsNewestFirst);
      this.finished.all.mergeFirstPage(page.completed, page.next_cursor);
      for (const category of SESSION_CATEGORIES) {
        if (this.finished[category].loaded) void this.finished[category].loadFirst();
      }
      this.listError = null;
      this.loaded = true;
    } catch (err) {
      this.listError = userErrorMessage(err, { action: "Couldn't load what's running." });
    } finally {
      this.refreshing = false;
    }
    if (this.refreshAgain) {
      this.refreshAgain = false;
      await this.refresh();
    }
  }

  showFinished(kind: FinishedKind): void {
    this.finishedKind = kind;
    const runs = this.finished[kind];
    if (!runs.loaded) void runs.loadFirst();
  }

  async refreshOutbound(): Promise<void> {
    try {
      this.outbound = await fetchOutboundA2aTasks(requireAgent(this.deps.agent));
      this.outboundError = null;
    } catch (err) {
      this.outboundError = userErrorMessage(err, {
        action: "Couldn't load the tasks sent to other agents.",
      });
    }
  }

  resync(): void {
    void this.refresh();
  }

  findRun(runId: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.run_id === runId) ??
      this.completedRuns().find((s) => s.run_id === runId)
    );
  }

  findByAddress(address: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.address === address) ??
      this.completedRuns()
        .filter((s) => s.address === address)
        .sort(compareRunsNewestFirst)[0]
    );
  }

  startFailure(address: string): SessionStartFailure | undefined {
    return this.startFailures.get(address);
  }

  async resolveRun(address: string, runId: string | null): Promise<string | null> {
    if (runId) return runId;
    const known = this.findByAddress(address);
    if (known) return known.run_id;
    const failure = this.startFailures.get(address);
    const noRecord = failure
      ? { message: failure.error, details: failure.details ?? undefined }
      : undefined;
    return lookUpNewestRun(requireAgent(this.deps.agent), address, noRecord);
  }

  async stop(address: string): Promise<void> {
    if (this.stopping.has(address)) return;
    this.stopping.add(address);
    try {
      await stopSession(requireAgent(this.deps.agent), address);
    } catch (err) {
      this.stopping.delete(address);
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't stop ${address}.`,
          notFound: "It had already finished.",
        }),
      );
    }
  }

  applyOutbound(task: OutboundA2aTaskSummary): void {
    const rest = this.outbound.filter((t) => t.task_id !== task.task_id);
    if (!task.open) {
      this.outbound = rest;
      return;
    }
    this.outbound = [task, ...rest].sort(
      (a, b) => Date.parse(b.started_at) - Date.parse(a.started_at),
    );
  }

  handleFrame(frame: SessionFrame): void {
    switch (frame.type) {
      case "session_outbound_a2a_task":
        this.applyOutbound(frame.task);
        return;
      case "session_started":
        this.startFailures.delete(frame.session.address);
        this.live = [frame.session, ...this.live.filter((s) => s.run_id !== frame.session.run_id)];
        return;
      case "session_start_failed":
        this.startFailures.set(frame.address, {
          error: frame.error,
          details: frame.error_details ?? null,
        });
        return;
      case "session_message_delivered":
      case "session_stop_requested":
      case "session_command_failed":
        return;
      case "session_memory_merged":
        this.handleMemoryMerged(frame);
        return;
      case "session_state_changed":
      case "session_completed":
      case "session_turn_started":
      case "session_turn_ended":
      case "session_turn_usage":
      case "session_tool_call":
      case "session_tool_result":
      case "session_broadcast_response":
      case "session_response":
      case "session_error":
      case "session_user_message":
        this.handleRunFrame(frame);
        return;
      default:
        frame satisfies never;
    }
  }

  private handleRunFrame(frame: RunFrame): void {
    const live = this.live.find((s) => s.run_id === frame.run_id);
    if (!live) this.scheduleRefresh();

    switch (frame.type) {
      case "session_state_changed":
        if (live) live.state = frame.state;
        break;
      case "session_completed":
        this.handleCompleted(frame, live);
        break;
      case "session_error":
        this.errors.set(frame.run_id, frame.message);
        break;
      case "session_turn_usage":
        if (live && frame.session_totals) live.usage = frame.session_totals;
        break;
      case "session_turn_started":
      case "session_turn_ended":
      case "session_tool_call":
      case "session_tool_result":
      case "session_broadcast_response":
      case "session_response":
      case "session_user_message":
        break;
      case "session_memory_merged":
        break;
    }
  }

  private handleCompleted(
    frame: Extract<SessionFrame, { type: "session_completed" }>,
    live: SessionSummary | undefined,
  ): void {
    this.errors.delete(frame.run_id);
    if (!live) return;
    this.stopping.delete(live.address);
    this.live = this.live.filter((s) => s.run_id !== frame.run_id);
    const finished: SessionSummary = {
      ...$state.snapshot(live),
      state: "completed",
      completed_at: isoNow(),
      outcome: frame.status,
      error: frame.error,
      error_details: frame.error_details ?? null,
      memory_pending: frame.memory_pending,
    };
    this.finished.all.prepend(finished);
    const kind = this.finished[finished.category];
    if (kind.loaded) kind.prepend(finished);
  }

  private handleMemoryMerged(
    frame: Extract<SessionFrame, { type: "session_memory_merged" }>,
  ): void {
    for (const run of this.completedRuns()) {
      if (run.run_id !== frame.run_id) continue;
      run.episode_id = frame.episode_id;
      run.memory_pending = false;
    }
  }

  private completedRuns(): SessionSummary[] {
    return [this.finished.all, ...SESSION_CATEGORIES.map((c) => this.finished[c])].flatMap(
      (runs) => runs.runs,
    );
  }

  private scheduleRefresh(): void {
    if (this.refreshTimer) return;
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      void this.refresh();
    }, REFRESH_DEBOUNCE_MS);
  }
}

export function unreachableAgentMessage(err: unknown): string | null {
  if (!(err instanceof ApiError) || err.status !== 502) return null;
  try {
    const body: unknown = JSON.parse(err.body);
    if (
      typeof body === "object" &&
      body !== null &&
      "code" in body &&
      body.code === "unreachable" &&
      "error" in body &&
      typeof body.error === "string"
    ) {
      return body.error;
    }
  } catch {}
  return null;
}

export function compareRunsNewestFirst(a: SessionSummary, b: SessionSummary): number {
  const byStart = Date.parse(b.started_at) - Date.parse(a.started_at);
  if (byStart !== 0) return byStart;
  if (a.run_id === b.run_id) return 0;
  return a.run_id > b.run_id ? -1 : 1;
}
