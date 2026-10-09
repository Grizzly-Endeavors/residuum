// ── Agent sessions store (Svelte 5 runes) ────────────────────────────
//
// What the Activity place lists for the bound agent: its live runs, its
// finished runs (paged, for every kind or one), and the tasks it sent to
// other agents, from the agent's session routes and kept current by its
// `session_*` WebSocket frames, which the coordinator routes here. One run
// in detail is the session panel's (`session-run.svelte.ts`). The one
// crossover is a session's message to the main agent, handed back to the
// coordinator to show in the main chat.

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

/** Every server frame that belongs to the sessions surface. */
export type SessionFrame = Extract<ServerMessage, { type: `session_${string}` }>;

/** Frames that describe activity inside one run. */
export type RunFrame = Extract<SessionFrame, { run_id: string }>;

export function isSessionFrame(msg: ServerMessage): msg is SessionFrame {
  return msg.type.startsWith("session_");
}

/** What the Finished list shows: one kind of run, or every kind. */
export type FinishedKind = SessionCategory | "all";

/** Completed runs fetched per page. */
const PAGE_SIZE = 25;

/** Coalesces bursts of frames for unknown runs into one listing refresh. */
const REFRESH_DEBOUNCE_MS = 250;

// ── Completed runs (one kind, or all) ────────────────────────────────

/** The completed runs of one kind (or every kind) loaded so far, paged from the server. */
export class CompletedRuns {
  /** Runs loaded so far, newest first. */
  runs = $state<SessionSummary[]>([]);
  /** Cursor for the next page, or `null` on the last. */
  nextCursor = $state<string | null>(null);
  /** The first page has arrived. */
  loaded = $state(false);
  /** Why the first page couldn't load, if it couldn't. */
  error = $state<string | null>(null);
  loadingMore = $state(false);

  constructor(
    /** The kind these runs are, `null` for every kind. */
    readonly category: SessionCategory | null,
    /** The agent whose runs these are, `null` for a store with no agent bound. */
    private readonly agent: string | null,
  ) {}

  private query(before?: string): Parameters<typeof fetchSessions>[1] {
    return { category: this.category ?? undefined, before, limit: PAGE_SIZE };
  }

  /** Load the first page, keeping runs paged in beyond it. A failure lands in `error`. */
  async loadFirst(): Promise<void> {
    try {
      const page = await fetchSessions(requireAgent(this.agent), this.query());
      this.mergeFirstPage(page.completed, page.next_cursor);
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't load the finished runs." });
    }
  }

  /** Load the next page of these completed runs. */
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

  /** Put a run that just finished at the top. */
  prepend(run: SessionSummary): void {
    this.runs = [run, ...this.runs.filter((s) => s.run_id !== run.run_id)];
  }

  /**
   * Replace the head of the list with a fresh first page, keeping runs
   * paged in beyond it. Runs are matched by id and kept only if they sort
   * after the page's last run in the server's order (newest start first,
   * ties broken by run id), so runs sharing a start time survive.
   */
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

// ── Sessions store ───────────────────────────────────────────────────

export interface SessionsStoreDeps {
  /** The agent these sessions belong to, `null` before one is bound. */
  agent: string | null;
  /** Show a session's message to the main agent in the main chat. */
  pushToMain: (from: string, runId: string, content: string, category: string | null) => void;
}

/**
 * The newest run, live first, of `agent`'s session at `address`, asked of the
 * server. Null, after telling the user, when there is none or the lookup fails.
 */
export async function lookUpNewestRun(agent: string, address: string): Promise<string | null> {
  try {
    const page = await fetchSessions(agent, { address, limit: 1 });
    const run = page.live[0] ?? page.completed[0];
    if (run) return run.run_id;
    notifications.surface("error", `There's no record of the session ${address}.`);
  } catch (err) {
    notifications.surface(
      "error",
      userErrorMessage(err, { action: `Couldn't open the session ${address}.` }),
    );
  }
  return null;
}

export class SessionsStore {
  /** Live runs (forking, running, idle, completing), newest first. */
  live = $state<SessionSummary[]>([]);
  /** Completed runs loaded so far, for every kind and per kind. */
  readonly finished: Record<FinishedKind, CompletedRuns>;
  /** The kind the Finished list shows. */
  finishedKind = $state<FinishedKind>("all");
  /** The listing has loaded at least once. */
  loaded = $state(false);
  listError = $state<string | null>(null);
  /** Latest error per live run, shown on its row. */
  errors = new SvelteMap<string, string>();
  /**
   * Addresses with a stop requested but not yet resolved, for a row's stop
   * button in Activity to show it.
   */
  stopping = new SvelteSet<string>();
  /** Open tasks the agent sent to remote agents, newest first. */
  outbound = $state<OutboundA2aTaskSummary[]>([]);
  /** Why the outbound task list couldn't load, if it couldn't. */
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

  /** The agent these sessions belong to, `null` before one is bound. */
  get agent(): string | null {
    return this.deps.agent;
  }

  /** What the agent has running: its live sessions and the tasks it has open on remote agents. */
  get runningCount(): number {
    return this.live.length + this.outbound.length;
  }

  // ── Listing ──────────────────────────────────────────────────────

  /**
   * Reload live sessions and the first page of finished runs, for every kind
   * and for each kind already shown. Runs paged in beyond the first page are
   * kept, so a refresh doesn't collapse the list the user is reading.
   */
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

  /** Show one kind of finished run, or every kind, loading its first page when needed. */
  showFinished(kind: FinishedKind): void {
    this.finishedKind = kind;
    const runs = this.finished[kind];
    if (!runs.loaded) void runs.loadFirst();
  }

  /** Reload the tasks sent to remote agents. Failure only affects that list. */
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

  /** Resynchronize after the WebSocket (re)connects: frames may have been missed. */
  resync(): void {
    void this.refresh();
  }

  findRun(runId: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.run_id === runId) ??
      this.completedRuns().find((s) => s.run_id === runId)
    );
  }

  /** The newest known run at `address`, live runs first. */
  findByAddress(address: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.address === address) ??
      this.completedRuns()
        .filter((s) => s.address === address)
        .sort(compareRunsNewestFirst)[0]
    );
  }

  /**
   * The run to show for a session address: `runId` when known, else its newest
   * run (live first), looked up on the server if it isn't loaded. Null, after
   * telling the user, when there is none.
   */
  async resolveRun(address: string, runId: string | null): Promise<string | null> {
    if (runId) return runId;
    const known = this.findByAddress(address);
    if (known) return known.run_id;
    return lookUpNewestRun(requireAgent(this.deps.agent), address);
  }

  // ── Commands ─────────────────────────────────────────────────────

  /** Stop the session at `address`. Its frames show the stop; a failure is a toast. */
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

  /** Put a task's latest state in the list, or take it out once it ended. */
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

  // ── Frames ───────────────────────────────────────────────────────

  handleFrame(frame: SessionFrame): void {
    switch (frame.type) {
      case "session_outbound_a2a_task":
        this.applyOutbound(frame.task);
        return;
      case "session_started":
        this.live = [frame.session, ...this.live.filter((s) => s.run_id !== frame.session.run_id)];
        return;
      case "session_message_delivered":
      case "session_stop_requested":
      case "session_command_failed":
        // Replies to socket commands, which this page sends over HTTP instead.
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
      case "session_message_to_main":
        this.handleRunFrame(frame);
        return;
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
      case "session_message_to_main":
        this.deps.pushToMain(
          frame.address,
          frame.run_id,
          frame.content,
          (live ?? this.findByAddress(frame.address))?.category ?? null,
        );
        break;
      case "session_turn_started":
      case "session_turn_ended":
      case "session_tool_call":
      case "session_tool_result":
      case "session_broadcast_response":
      case "session_response":
        // A run's output is the session panel's, through the hub's relay.
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
      episode_id: frame.episode_id,
      outcome: frame.status,
      error: frame.error,
      error_details: frame.error_details,
    };
    this.finished.all.prepend(finished);
    const kind = this.finished[finished.category];
    if (kind.loaded) kind.prepend(finished);
  }

  // ── Private ──────────────────────────────────────────────────────

  /** Every completed run loaded so far. */
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

/**
 * The server's explanation when an outbound task's stop failed because its
 * agent couldn't be reached (`502` with code `unreachable`), else `null`.
 */
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
  } catch {
    // Not our JSON error body (a proxy's error page); handled as a plain failure.
  }
  return null;
}

/**
 * Order runs as the server lists completed runs: newest start first, then
 * run id descending. Negative when `a` comes before `b`.
 */
export function compareRunsNewestFirst(a: SessionSummary, b: SessionSummary): number {
  const byStart = Date.parse(b.started_at) - Date.parse(a.started_at);
  if (byStart !== 0) return byStart;
  if (a.run_id === b.run_id) return 0;
  return a.run_id > b.run_id ? -1 : 1;
}
