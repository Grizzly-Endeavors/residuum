// ── Agent sessions store (Svelte 5 runes) ────────────────────────────
//
// Owns everything the sessions sidebar and session view show: the listing
// from `GET /api/sessions`, live `session_*` WebSocket frames, the run being
// viewed, and the sidebar's commands (message, stop). Session frames are
// routed here by the WebSocket coordinator and never reach the main
// `FeedStore` — the one crossover is a session's message to the main agent,
// which is handed back to the coordinator to show in the main chat.

import { SvelteMap } from "svelte/reactivity";
import { fetchSessionTranscript, fetchSessions } from "./api";
import { nextFeedId } from "./feed-id";
import { appendToolCall, applyToolResult, convertHistoryMessages } from "./feed-items";
import { notifications } from "./notifications.svelte";
import { deliveryOutcomeText, runOutcomeText } from "./session-format";
import type {
  ClientMessage,
  FeedItem,
  ServerMessage,
  SessionRunStatus,
  SessionSummary,
  ToolCallState,
} from "./types";

/** Every server frame that belongs to the sessions surface. */
export type SessionFrame = Extract<ServerMessage, { type: `session_${string}` }>;

/** Frames that describe activity inside one run. */
type RunFrame = Extract<SessionFrame, { run_id: string }>;

export function isSessionFrame(msg: ServerMessage): msg is SessionFrame {
  return msg.type.startsWith("session_");
}

/** Completed runs fetched per page. */
const PAGE_SIZE = 25;

/** Coalesces bursts of frames for unknown runs into one listing refresh. */
const REFRESH_DEBOUNCE_MS = 250;

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

// ── Session view ─────────────────────────────────────────────────────

/** One run shown in the main pane: its transcript plus live frames. */
export class SessionView {
  runId = $state<string>("");
  summary = $state<SessionSummary | null>(null);
  items = $state<FeedItem[]>([]);
  loading = $state(true);
  loadError = $state<string | null>(null);
  /** A stop was requested and the run hasn't finished yet. */
  stopRequested = $state(false);
  /**
   * Set when a message from this view started (or will start) a new run at
   * the same address; the view follows that run when it appears.
   */
  followAddress: string | null = null;

  private pendingTools = new SvelteMap<string, ToolCallState>();
  /** Frames that arrived while the transcript was loading. */
  private buffered: RunFrame[] = [];

  constructor(runId: string, summary: SessionSummary | null) {
    this.runId = runId;
    this.summary = summary;
  }

  /** Load (or reload) the transcript, then replay frames that raced it. */
  async load(): Promise<void> {
    this.loading = true;
    this.loadError = null;
    this.buffered = [];
    const runId = this.runId;
    try {
      const transcript = await fetchSessionTranscript(runId);
      if (runId !== this.runId) return;
      this.summary = transcript.session;
      this.pendingTools.clear();
      this.items = convertHistoryMessages(transcript.messages, { mode: "session" });
    } catch (err) {
      if (runId !== this.runId) return;
      this.loadError = `Couldn't load this session's transcript. ${errorText(err)}`;
      return;
    } finally {
      if (runId === this.runId) this.loading = false;
    }
    const raced = this.buffered;
    this.buffered = [];
    for (const frame of raced) this.applyFrame(frame, { dedupe: true });
  }

  /**
   * Apply a live frame for this run. `dedupe` is set when replaying frames
   * that arrived during the transcript fetch, which may already be in it.
   */
  applyFrame(frame: RunFrame, opts: { dedupe: boolean } = { dedupe: false }): void {
    if (this.loading) {
      this.buffered.push(frame);
      return;
    }
    switch (frame.type) {
      case "session_tool_call":
        if (opts.dedupe && this.findToolCall(frame.id)) return;
        appendToolCall(this.items, this.pendingTools, frame);
        break;
      case "session_tool_result": {
        if (!this.pendingTools.has(frame.tool_call_id)) {
          const existing = this.findToolCall(frame.tool_call_id);
          if (existing && existing.result === undefined) {
            this.pendingTools.set(frame.tool_call_id, existing);
          }
        }
        applyToolResult(this.pendingTools, frame);
        break;
      }
      case "session_broadcast_response":
      case "session_response":
        if (!frame.content) return;
        if (opts.dedupe && this.lastAssistantContent() === frame.content) return;
        this.items.push({ id: nextFeedId(), kind: "assistant", content: frame.content });
        break;
      case "session_error":
        this.pushStatus("error", frame.message);
        break;
      case "session_completed":
        this.stopRequested = false;
        this.pushStatus(
          frame.status === "failed" ? "error" : "info",
          runOutcomeText(frame.status, frame.error),
        );
        break;
      case "session_state_changed":
      case "session_turn_started":
      case "session_turn_ended":
      case "session_message_to_main":
        break;
    }
  }

  pushStatus(tone: "info" | "error", content: string): void {
    this.items.push({ id: nextFeedId(), kind: "status", tone, content });
  }

  pushOwnerMessage(content: string): void {
    this.items.push({ id: nextFeedId(), kind: "user", content });
  }

  /** Continue this view in a new run of the same session. */
  follow(summary: SessionSummary): void {
    this.followAddress = null;
    this.runId = summary.run_id;
    this.summary = summary;
    this.stopRequested = false;
    this.pendingTools.clear();
    this.items.push({ id: nextFeedId(), kind: "divider", variant: "day", label: "new run" });
  }

  private findToolCall(id: string): ToolCallState | undefined {
    for (const item of this.items) {
      if (item.kind !== "tool-group") continue;
      const call = item.calls.find((c) => c.id === id);
      if (call) return call;
    }
    return undefined;
  }

  private lastAssistantContent(): string | null {
    for (let i = this.items.length - 1; i >= 0; i--) {
      const item = this.items[i];
      if (item?.kind === "assistant") return item.content;
    }
    return null;
  }
}

// ── Sessions store ───────────────────────────────────────────────────

interface PendingCommand {
  kind: "message" | "stop";
  address: string;
}

export interface SessionsStoreDeps {
  /** Send a client frame over the WebSocket. */
  send: (msg: ClientMessage) => void;
  /** Show a session's message to the main agent in the main chat. */
  pushToMain: (from: string, runId: string, content: string, category: string | null) => void;
}

export class SessionsStore {
  /** Live runs (forking, running, idle, completing), newest first. */
  live = $state<SessionSummary[]>([]);
  /** Completed runs loaded so far, newest first. */
  completed = $state<SessionSummary[]>([]);
  /** Cursor for the next page of completed runs, or `null` on the last. */
  nextCursor = $state<string | null>(null);
  /** The listing has loaded at least once. */
  loaded = $state(false);
  loadingMore = $state(false);
  listError = $state<string | null>(null);
  /** How runs ended, for those that finished while this page was open. */
  outcomes = new SvelteMap<string, { status: SessionRunStatus; error: string | null }>();
  /** Latest error per live run, flagged in the sidebar. */
  errors = new SvelteMap<string, string>();
  /** The run shown in the main pane, if any. */
  view = $state<SessionView | null>(null);
  /** Clock for elapsed times; ticked by the app while sessions are live. */
  now = $state(Date.now());

  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- bookkeeping only, never rendered
  private pending = new Map<string, PendingCommand>();
  private commandCounter = 0;
  private refreshing = false;
  private refreshAgain = false;
  private refreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly deps: SessionsStoreDeps) {}

  // ── Listing ──────────────────────────────────────────────────────

  /**
   * Reload live sessions and the first page of completed runs. Completed
   * runs already paged in beyond the first page are kept, so a refresh
   * doesn't collapse the list the user is reading.
   */
  async refresh(): Promise<void> {
    if (this.refreshing) {
      this.refreshAgain = true;
      return;
    }
    this.refreshing = true;
    try {
      const page = await fetchSessions({ limit: PAGE_SIZE });
      this.live = page.live;
      this.mergeFirstPage(page.completed, page.next_cursor);
      this.listError = null;
      this.loaded = true;
      this.syncViewSummary();
    } catch (err) {
      this.listError = `Couldn't load sessions. ${errorText(err)}`;
    } finally {
      this.refreshing = false;
    }
    if (this.refreshAgain) {
      this.refreshAgain = false;
      await this.refresh();
    }
  }

  /** Load the next page of completed runs. */
  async loadMore(): Promise<void> {
    const cursor = this.nextCursor;
    if (!cursor || this.loadingMore) return;
    this.loadingMore = true;
    try {
      const page = await fetchSessions({ before: cursor, limit: PAGE_SIZE });
      // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive scratch
      const known = new Set(this.completed.map((s) => s.run_id));
      this.completed.push(...page.completed.filter((s) => !known.has(s.run_id)));
      this.nextCursor = page.next_cursor;
    } catch (err) {
      notifications.surface("error", `Couldn't load more finished sessions. ${errorText(err)}`);
    } finally {
      this.loadingMore = false;
    }
  }

  /** Resynchronize after the WebSocket (re)connects: frames may have been missed. */
  resync(): void {
    void this.refresh();
    const view = this.view;
    if (view && !view.loading && view.summary?.state !== "completed") void view.load();
  }

  findRun(runId: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.run_id === runId) ?? this.completed.find((s) => s.run_id === runId)
    );
  }

  /** The newest known run at `address`, live runs first. */
  findByAddress(address: string): SessionSummary | undefined {
    return (
      this.live.find((s) => s.address === address) ??
      this.completed.find((s) => s.address === address)
    );
  }

  // ── Navigation ───────────────────────────────────────────────────

  /** Show a run in the main pane. */
  openRun(runId: string): void {
    if (this.view?.runId === runId) return;
    const view = new SessionView(runId, this.findRun(runId) ?? null);
    this.view = view;
    void view.load();
  }

  /**
   * Show a session by address: `runId` when known, else its newest run
   * (live first), looked up on the server if it isn't loaded.
   */
  async openAddress(address: string, runId: string | null): Promise<void> {
    if (runId) {
      this.openRun(runId);
      return;
    }
    const known = this.findByAddress(address);
    if (known) {
      this.openRun(known.run_id);
      return;
    }
    try {
      const page = await fetchSessions({ address, limit: 1 });
      const run = page.live[0] ?? page.completed[0];
      if (run) {
        this.openRun(run.run_id);
      } else {
        notifications.surface("error", `There's no record of the session ${address}.`);
      }
    } catch (err) {
      notifications.surface("error", `Couldn't open the session ${address}. ${errorText(err)}`);
    }
  }

  /** Return the main pane to the main chat. */
  closeView(): void {
    this.view = null;
  }

  // ── Commands ─────────────────────────────────────────────────────

  /** Message the session at `address` as the owner. */
  sendMessage(address: string, content: string): void {
    const id = this.commandId();
    this.pending.set(id, { kind: "message", address });
    this.deps.send({ type: "session_send_message", id, address, content });
    const view = this.viewFor(address);
    view?.pushOwnerMessage(content);
  }

  /** Stop the session at `address`. */
  stop(address: string): void {
    const id = this.commandId();
    this.pending.set(id, { kind: "stop", address });
    this.deps.send({ type: "session_stop", id, address });
    const view = this.viewFor(address);
    if (view) view.stopRequested = true;
  }

  // ── Frames ───────────────────────────────────────────────────────

  handleFrame(frame: SessionFrame): void {
    switch (frame.type) {
      case "session_started":
        this.handleStarted(frame.session);
        return;
      case "session_message_delivered":
      case "session_stop_requested":
      case "session_command_failed":
        this.handleCommandReply(frame);
        return;
      case "session_state_changed":
      case "session_completed":
      case "session_turn_started":
      case "session_turn_ended":
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

  private handleStarted(session: SessionSummary): void {
    this.live = [session, ...this.live.filter((s) => s.run_id !== session.run_id)];
    const view = this.view;
    if (view?.followAddress === session.address) view.follow(session);
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
        break;
    }

    const view = this.view;
    if (view?.runId === frame.run_id) {
      view.applyFrame(frame);
      this.syncViewSummary();
    }
  }

  private handleCompleted(
    frame: Extract<SessionFrame, { type: "session_completed" }>,
    live: SessionSummary | undefined,
  ): void {
    this.outcomes.set(frame.run_id, { status: frame.status, error: frame.error });
    this.errors.delete(frame.run_id);
    if (!live) return;
    this.live = this.live.filter((s) => s.run_id !== frame.run_id);
    const finished: SessionSummary = {
      ...$state.snapshot(live),
      state: "completed",
      // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a one-off timestamp
      completed_at: new Date().toISOString(),
      episode_id: frame.episode_id,
    };
    this.completed = [finished, ...this.completed.filter((s) => s.run_id !== frame.run_id)];
  }

  private handleCommandReply(
    frame: Extract<
      SessionFrame,
      {
        type: "session_message_delivered" | "session_stop_requested" | "session_command_failed";
      }
    >,
  ): void {
    const command = this.pending.get(frame.id);
    this.pending.delete(frame.id);
    const view = this.viewFor(frame.address);

    switch (frame.type) {
      case "session_message_delivered": {
        const text = deliveryOutcomeText(frame.outcome);
        if (view) {
          view.pushStatus("info", text);
          if (frame.outcome !== "live") view.followAddress = frame.address;
        } else {
          notifications.surface("notice", `${frame.address}: ${text}`);
        }
        break;
      }
      case "session_stop_requested":
        if (view) view.pushStatus("info", "Stopping this session…");
        break;
      case "session_command_failed": {
        const action = command?.kind === "stop" ? "Couldn't stop it" : "Couldn't send it";
        if (view) {
          if (command?.kind === "stop") view.stopRequested = false;
          view.pushStatus("error", `${action}. ${frame.message}`);
        } else {
          notifications.surface("error", `${frame.address}: ${frame.message}`);
        }
        break;
      }
    }
  }

  // ── Private ──────────────────────────────────────────────────────

  private viewFor(address: string): SessionView | null {
    const view = this.view;
    return view?.summary?.address === address ? view : null;
  }

  private commandId(): string {
    this.commandCounter++;
    return `session-cmd-${this.commandCounter}`;
  }

  /** Keep the viewed run's header in step with the listing. */
  private syncViewSummary(): void {
    const view = this.view;
    if (!view) return;
    const known = this.findRun(view.runId);
    if (!known || !view.summary) return;
    view.summary.state = known.state;
    view.summary.completed_at = known.completed_at;
    view.summary.episode_id = known.episode_id;
  }

  private scheduleRefresh(): void {
    if (this.refreshTimer) return;
    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      void this.refresh();
    }, REFRESH_DEBOUNCE_MS);
  }

  private mergeFirstPage(first: SessionSummary[], firstCursor: string | null): void {
    const oldCursor = this.nextCursor;
    const last = first[first.length - 1];
    if (firstCursor === null || !last || this.completed.length <= first.length) {
      this.completed = first;
      this.nextCursor = firstCursor;
      return;
    }
    const lastStart = Date.parse(last.started_at);
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive scratch
    const inFirst = new Set(first.map((s) => s.run_id));
    const tail = this.completed.filter(
      (s) => !inFirst.has(s.run_id) && Date.parse(s.started_at) < lastStart,
    );
    this.completed = [...first, ...tail];
    this.nextCursor = tail.length ? oldCursor : firstCursor;
  }
}
