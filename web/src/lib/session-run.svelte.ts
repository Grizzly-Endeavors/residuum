// ── One session run in the context panel (Svelte 5 runes) ────────────
//
// What the session panel shows: a run's transcript, its live frames, and the
// commands that act on it. It works for a run on any agent, so it reads
// nothing from the bound agent's socket: the transcript loads over HTTP, live
// frames come through the hub socket's session relay, and messages and stops
// go to the agent's session routes. The panel host owns it, so it outlives
// the panel's frame drawing its content again at another width.

import { SvelteMap } from "svelte/reactivity";
import { fetchSessionTranscript, messageSession, stopSession } from "./api";
import { userErrorMessage } from "./errors";
import { nextFeedId } from "./feed-id";
import {
  appendToolCall,
  applyToolResult,
  convertHistoryMessages,
  settlePendingCalls,
} from "./feed-items";
import { hub } from "./hub.svelte";
import type { HubClientMessage, HubServerMessage } from "./hub-types";
import { ObservedTurns, type TurnEnding } from "./observed-turns.svelte";
import { deliveryOutcomeText, runOutcomeText } from "./session-format";
import { isSessionFrame, type RunFrame, type SessionFrame } from "./sessions.svelte";
import { isoNow } from "./time";
import type { FeedItem, SessionSummary, ToolCallState } from "./types";

/** The hub socket, as far as a run follows its session through it. */
export interface SessionRelayLink {
  readonly connected: boolean;
  send: (msg: HubClientMessage) => void;
  onFrame: (listener: (msg: HubServerMessage) => void) => () => void;
}

const hubRelay: SessionRelayLink = {
  get connected() {
    return hub.transport.status === "connected";
  },
  send: (msg) => {
    hub.transport.send(msg);
  },
  onFrame: (listener) => hub.onFrame(listener),
};

/** What is known of a run before its transcript arrives. */
export interface KnownRun {
  summary?: SessionSummary | null;
  address?: string | null;
}

function frameAddress(frame: SessionFrame): string | null {
  if (frame.type === "session_started") return frame.session.address;
  return "address" in frame ? frame.address : null;
}

export class SessionRun {
  runId = $state("");
  summary = $state<SessionSummary | null>(null);
  items = $state<FeedItem[]>([]);
  /** The transcript has arrived at least once. */
  loaded = $state(false);
  loadError = $state<string | null>(null);
  /** The turn in flight, whose items are tagged with it; null between turns. */
  activeTurnId = $state<string | null>(null);
  /** What the panel saw of each turn while it ran, for the activity line. */
  readonly observed = new ObservedTurns();
  /** The message box's text, kept here so it survives the panel drawing again. */
  draft = $state("");
  sending = $state(false);
  /** A stop was asked for and the run hasn't finished yet. */
  stopping = $state(false);

  private address: string | null;
  /** The relay connection this run is subscribed on has acknowledged it. */
  private subscribed = false;
  /** A message sent from here may start a new run at this address; the panel follows it. */
  private followNextRun = false;
  private pendingTools = new SvelteMap<string, ToolCallState>();
  /** Frames for this run that arrived while its transcript was loading. */
  private buffered: RunFrame[] = [];
  /** Status lines this page wrote while the transcript was loading; no transcript holds them. */
  private notes: FeedItem[] = [];
  private loading = false;
  /** Identifies the latest load; an older one finishing late is ignored. */
  private loadToken = 0;
  /** Where the turn in flight began, for tagging its message with the turn id (Undo this turn). */
  private turnStart: number | null = null;

  constructor(
    readonly agent: string,
    runId: string,
    known: KnownRun = {},
    private readonly relay: SessionRelayLink = hubRelay,
  ) {
    this.runId = runId;
    this.summary = known.summary ?? null;
    this.address = known.summary?.address ?? known.address ?? null;
  }

  /** The run may still produce frames. */
  get live(): boolean {
    return this.summary?.state !== "completed";
  }

  /** Start following the run: load it and subscribe to its frames. Returns a function that stops. */
  open(): () => void {
    const stopListening = this.relay.onFrame((msg) => {
      this.handleHubFrame(msg);
    });
    this.subscribe();
    void this.load();
    return () => {
      stopListening();
      if (this.address !== null && this.relay.connected) {
        this.relay.send({ type: "unsubscribe_session", agent: this.agent, address: this.address });
      }
    };
  }

  /** Load (or load again) the run's transcript, then replay frames that raced it. */
  async load(): Promise<void> {
    const token = ++this.loadToken;
    const runId = this.runId;
    this.loading = true;
    this.loadError = null;
    this.buffered = this.buffered.filter((frame) => frame.run_id === runId);
    let transcript;
    try {
      transcript = await fetchSessionTranscript(this.agent, runId);
    } catch (err) {
      if (token !== this.loadToken) return;
      this.loadError = userErrorMessage(err, {
        action: "Couldn't load this session's transcript.",
        notFound: "Its history isn't available. It may be from before a restart.",
      });
      this.buffered = [];
      this.loading = false;
      this.items.push(...this.notes.splice(0));
      return;
    }
    if (token !== this.loadToken) return;
    this.summary = transcript.session;
    this.pendingTools.clear();
    this.turnStart = null;
    this.items = convertHistoryMessages(transcript.messages, { mode: "session" });
    this.items.push(...this.notes.splice(0));
    // The transcript renders every finished turn again, without timing. A
    // turn still running goes on from here; what it did so far is in the
    // transcript.
    this.observed.clear(this.activeTurnId);
    if (this.activeTurnId !== null) this.turnStart = this.items.length;
    this.loading = false;
    this.loaded = true;
    const raced = this.buffered;
    this.buffered = [];
    for (const frame of raced) this.applyFrame(frame, true);
    if (this.address === null) {
      this.address = transcript.session.address;
      this.subscribe();
    }
  }

  // ── Commands ─────────────────────────────────────────────────────

  /** Send the message box's text to the session: to this run, or a new one if it finished. */
  async send(): Promise<void> {
    const content = this.draft.trim();
    const address = this.address;
    if (content === "" || address === null || this.sending) return;
    this.sending = true;
    this.draft = "";
    this.turnStart ??= this.items.length;
    // With a turn running, the session takes the message into it at its next checkpoint.
    const running = this.activeTurnId;
    this.items.push({
      id: nextFeedId(),
      kind: "user",
      content,
      ...(running === null ? {} : { turnId: running, midTurn: true }),
    });
    // The new run a finished session starts can announce itself before the reply.
    this.followNextRun = true;
    try {
      const outcome = await messageSession(this.agent, address, content);
      if (outcome === "live") this.followNextRun = false;
      this.pushStatus("info", deliveryOutcomeText(outcome));
    } catch (err) {
      this.followNextRun = false;
      if (this.draft === "") this.draft = content;
      this.pushStatus(
        "error",
        userErrorMessage(err, {
          action: "Couldn't send it.",
          notFound: "This session isn't known any more. It may be from before a restart.",
        }),
      );
    } finally {
      this.sending = false;
    }
  }

  /** Stop the run. Its frames show it finishing. */
  async stop(): Promise<void> {
    const address = this.address;
    if (address === null || this.stopping) return;
    this.stopping = true;
    if (this.activeTurnId !== null) this.observed.askStop(this.activeTurnId);
    try {
      await stopSession(this.agent, address);
      // Its completion may have arrived first.
      if (this.live) this.pushStatus("info", "Stopping this session…");
    } catch (err) {
      this.stopping = false;
      this.pushStatus(
        "error",
        userErrorMessage(err, {
          action: "Couldn't stop it.",
          notFound: "It had already finished.",
        }),
      );
    }
  }

  // ── Frames ───────────────────────────────────────────────────────

  private subscribe(): void {
    if (this.address === null || !this.relay.connected) return;
    this.relay.send({ type: "subscribe_session", agent: this.agent, address: this.address });
  }

  private handleHubFrame(msg: HubServerMessage): void {
    if (msg.type === "hub_boot") {
      // A new connection: subscriptions ended with the old one.
      this.subscribed = false;
      this.subscribe();
    } else if (
      msg.type === "subscribed" &&
      msg.agent === this.agent &&
      msg.address === this.address
    ) {
      this.subscribed = true;
      // Frames between the transcript's reading and now never reached this page.
      if (this.live) void this.load();
    } else if (msg.type === "session_relay_lagged") {
      if (this.subscribed) void this.load();
    } else if (msg.type === "session_frame" && msg.agent === this.agent) {
      if (isSessionFrame(msg.frame)) this.handleFrame(msg.frame);
    }
  }

  private handleFrame(frame: SessionFrame): void {
    if (this.address === null || frameAddress(frame) !== this.address) return;
    if (frame.type === "session_started") {
      if (this.followNextRun && frame.session.run_id !== this.runId) this.follow(frame.session);
      return;
    }
    if ("run_id" in frame && frame.run_id === this.runId) this.applyFrame(frame);
  }

  /**
   * Apply a live frame for this run. `dedupe` is set when replaying frames
   * that arrived during the transcript fetch, which may already be in it.
   */
  private applyFrame(frame: RunFrame, dedupe = false): void {
    if (this.loading) {
      this.buffered.push(frame);
      return;
    }
    const summary = this.summary;
    switch (frame.type) {
      case "session_state_changed":
        if (summary) summary.state = frame.state;
        break;
      case "session_tool_call":
        if (dedupe && this.findToolCall(frame.id)) return;
        this.joinTurn();
        appendToolCall(this.items, this.pendingTools, frame, this.activeTurnId ?? undefined);
        break;
      case "session_tool_result": {
        this.joinTurn();
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
        this.joinTurn(frame.type === "session_response" ? frame.turn_id : undefined);
        if (!frame.content) return;
        if (dedupe && this.items.some((i) => i.kind === "assistant" && i.content === frame.content))
          return;
        this.items.push({
          id: nextFeedId(),
          kind: "assistant",
          content: frame.content,
          ...(this.activeTurnId === null ? {} : { turnId: this.activeTurnId }),
        });
        break;
      case "session_error":
        this.pushStatus("error", frame.message, frame.details ?? undefined);
        break;
      case "session_completed":
        if (summary) {
          summary.state = "completed";
          summary.completed_at = isoNow();
          summary.outcome = frame.status;
          summary.error = frame.error;
          summary.error_details = frame.error_details;
          summary.episode_id = frame.episode_id;
        }
        // A run that ends mid-turn ends the turn with it: stopped when the
        // user asked, cut off otherwise.
        if (this.activeTurnId !== null) {
          const asked = this.observed.get(this.activeTurnId)?.stopAsked === true;
          this.closeTurn(this.activeTurnId, asked ? "stopped" : "interrupted");
        }
        this.stopping = false;
        this.pushStatus(
          frame.status === "failed" ? "error" : "info",
          runOutcomeText(frame.status, frame.error),
        );
        break;
      case "session_turn_started":
        this.turnStart ??= this.items.length;
        // The message that started it went out from here first.
        for (const item of this.items.slice(this.turnStart)) item.turnId = frame.turn_id;
        this.activeTurnId = frame.turn_id;
        this.observed.start(frame.turn_id);
        break;
      case "session_turn_ended":
        this.nameJoinedTurn(frame.turn_id);
        this.tagTurnStart(frame.turn_id);
        this.closeTurn(frame.turn_id);
        this.turnStart = null;
        break;
      case "session_turn_usage":
        this.joinTurn();
        if (summary && frame.session_totals) summary.usage = frame.session_totals;
        break;
      case "session_message_to_main":
        break;
    }
  }

  private pushStatus(tone: "info" | "error", content: string, details?: string): void {
    const note: FeedItem = { id: nextFeedId(), kind: "status", tone, content, details };
    if (this.loading) this.notes.push(note);
    else this.items.push(note);
  }

  /**
   * A frame of a turn arrived with no turn in flight: the panel opened, or
   * its transcript loaded, while the turn ran. It becomes the turn in flight,
   * joined partway, under a stand-in id until a frame names it.
   */
  private joinTurn(turnId?: string): void {
    if (this.activeTurnId !== null) {
      if (turnId !== undefined) this.nameJoinedTurn(turnId);
      return;
    }
    if (turnId !== undefined && this.observed.get(turnId)?.endedAt != null) return;
    this.activeTurnId = this.observed.join(turnId ?? null, null);
    this.turnStart = this.items.length;
  }

  /** A frame named the turn joined under a stand-in id: tag its items with the real one. */
  private nameJoinedTurn(turnId: string): void {
    const current = this.activeTurnId;
    if (current === null || current === turnId || !this.observed.isUnnamed(current)) return;
    for (const item of this.items) if (item.turnId === current) item.turnId = turnId;
    this.observed.rename(current, turnId);
    this.activeTurnId = turnId;
  }

  /** The turn ended: settle calls still waiting on results, and record how. */
  private closeTurn(turnId: string, ending?: TurnEnding): void {
    const how = this.observed.end(turnId, ending);
    settlePendingCalls(this.pendingTools, how === "finished" ? "done" : "stopped");
    if (this.activeTurnId === turnId) this.activeTurnId = null;
  }

  /** Tag the message that started the turn now ending, so it can offer Undo this turn. */
  private tagTurnStart(turnId: string): void {
    if (this.turnStart === null) return;
    const item = this.items[this.turnStart];
    if (item?.kind === "user") item.turn = { turnId, changed: null };
  }

  /**
   * Continue in a new run of the same session. Mid-load, the pending load is
   * superseded by one for the new run, whose transcript holds what started it.
   */
  private follow(next: SessionSummary): void {
    this.followNextRun = false;
    this.runId = next.run_id;
    this.summary = next;
    this.stopping = false;
    this.activeTurnId = null;
    this.pendingTools.clear();
    if (this.loading) {
      void this.load();
      return;
    }
    this.items.push({ id: nextFeedId(), kind: "divider", variant: "day", label: "New run" });
  }

  private findToolCall(id: string): ToolCallState | undefined {
    for (const item of this.items) {
      if (item.kind !== "tool-group") continue;
      const call = item.calls.find((c) => c.id === id);
      if (call) return call;
    }
    return undefined;
  }
}
