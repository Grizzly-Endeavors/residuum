// ── Schedule place state (Svelte 5 runes) ────────────────────────────
//
// The bound agent's pulses and scheduled actions, for the Schedule place.
// Loaded over REST (there's no dedicated WebSocket feed for pulse/action
// definitions) and refetched on signals the socket already carries: a change
// to HEARTBEAT.yml or scheduled_actions.json, which this store watches through
// the socket's watch registry, any `scheduled`-category session frame (a
// pulse or action fired, or finished), and a `pulse_fire_changed` frame —
// never on a bare poll timer. A `pulse_fire_changed` frame also carries the
// pulse's run in flight and its new outcome, which land at once, before the
// refetch that brings its next run time.

import { SvelteSet } from "svelte/reactivity";
import {
  ApiError,
  fetchScheduledPulses,
  fetchScheduledActions,
  setPulseEnabled as apiSetPulseEnabled,
  cancelScheduledAction as apiCancelScheduledAction,
  stopPulseFire,
  stopSession,
} from "./api";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";
import { requireAgent } from "./paths";
import type { ActionInfo, PulseInfo, ScheduledCurrentRun, ServerMessage } from "./types";
import type { WatchHandler, WatchOwner, WatchOwnerOptions } from "./watch-registry";

/** The files whose changes mean the pulses or the actions changed. */
const WATCHED_FILES = ["HEARTBEAT.yml", "scheduled_actions.json"];

/**
 * Where refetch signals come from: the bound agent's socket (`ws`), passed in
 * because the socket's coordinator resets this store and so imports it.
 */
export interface ScheduledSources {
  /** Observe every frame on the bound agent's socket. Returns a function that stops. */
  onFrame: (listener: (msg: ServerMessage) => void) => () => void;
  /** The bound agent's watch registry. */
  watches: { register: (handler: WatchHandler, options: WatchOwnerOptions) => WatchOwner };
}

// Plain `if` checks rather than a `switch` over the full `ServerMessage`
// union: only three of its ~30 frame types matter here, and the project's
// exhaustiveness lint requires every switch over a union to list every
// variant (no `default` shortcut), which would make this function's real
// point — spotting three specific frames — unreadable.
function isScheduledSessionFrame(msg: ServerMessage): boolean {
  if (msg.type === "session_started") return msg.session.category === "scheduled";
  // `session_state_changed` and `session_completed` carry only an address,
  // not a category. Refetching on every one of them (rather than
  // cross-referencing the sessions store by address) trades a little extra
  // fetching for staying correct without a second source of truth to keep
  // in sync. These two are what actually move "current run"/"last
  // outcome" — per-turn frames (turn_started, response, ...) don't, so
  // they're left out to avoid refetching on every tool call.
  return msg.type === "session_state_changed" || msg.type === "session_completed";
}

/** A change to one of a pulse's fires: its run in flight now, and its new outcome. */
type PulseFireFrame = Extract<ServerMessage, { type: "pulse_fire_changed" }>;

/**
 * What stopping `run` acts on: its fire, or its session when no fire tracks
 * it (a scheduled action's run, a pulse session a message resumed). `null`
 * for a run nothing can stop yet.
 */
export function stopTarget(run: ScheduledCurrentRun): string | null {
  return run.fire_id ?? run.address;
}

class ScheduledStore {
  pulses = $state<PulseInfo[]>([]);
  actions = $state<ActionInfo[]>([]);
  loading = $state(false);
  loaded = $state(false);
  /** Why the last load failed, in words for the user; `null` once one succeeds. */
  loadError = $state<string | null>(null);
  /** Names/ids currently being toggled or cancelled, to disable their controls. */
  readonly pending = new SvelteSet<string>();
  /**
   * The runs asked to stop that are still in flight, by `stopTarget`. A run
   * leaves once a frame or a load no longer shows it.
   */
  readonly stopping = new SvelteSet<string>();

  private sources: ScheduledSources | null = null;
  private unsubscribeFrame: (() => void) | null = null;
  private fileWatch: WatchOwner | null = null;
  private watchers = 0;
  /** Bumped on a reset, so a load begun for the previous agent can tell it is stale. */
  private generation = 0;
  /** The agent whose pulses and actions these are, `null` before one is bound. */
  private agent: string | null = null;
  /**
   * For each load in flight, the fire frames merged since it began. A load's
   * answer can predate them, so they are merged again on top of it.
   */
  private readonly framesDuringLoads = new Set<PulseFireFrame[]>();
  /** Numbers each load in the order it began. */
  private loadSeq = 0;
  /**
   * The number of the newest load whose answer was applied. Loads answer in
   * any order, so an older load's answer is dropped once a newer one landed.
   */
  private appliedSeq = 0;

  /**
   * Start watching for refetch signals. Call once per mounted view; pairs
   * with `stopWatching()`. Reference-counted so more than one open view
   * doesn't double-subscribe or tear down the other's subscription.
   */
  startWatching(sources: ScheduledSources): void {
    this.watchers++;
    if (this.sources) return;
    this.sources = sources;
    this.unsubscribeFrame = sources.onFrame((msg) => {
      if (msg.type === "pulse_fire_changed") {
        this.applyFireFrame(msg);
        // The fire moved the pulse's next run, which the frame doesn't carry.
        void this.load();
      } else if (isScheduledSessionFrame(msg)) {
        void this.load();
      }
    });
    this.watchFiles();
  }

  stopWatching(): void {
    this.watchers = Math.max(0, this.watchers - 1);
    if (this.watchers > 0) return;
    this.unsubscribeFrame?.();
    this.unsubscribeFrame = null;
    this.fileWatch?.release();
    this.fileWatch = null;
    this.sources = null;
  }

  /** Own a watch on the bound agent's schedule files, tied to that agent, while watching. */
  private watchFiles(): void {
    this.fileWatch?.release();
    this.fileWatch = null;
    if (this.sources === null || this.agent === null) return;
    const reload = (): void => void this.load();
    // A resync means changes were missed, so the files may have changed too.
    this.fileWatch = this.sources.watches.register(
      { changed: reload, resync: reload },
      { agent: this.agent },
    );
    this.fileWatch.set(WATCHED_FILES);
  }

  /**
   * Forget the bound agent's pulses and actions and take `agent` as the new
   * one (`null` for none). A view that is open reloads for it.
   */
  reset(agent: string | null): void {
    this.generation++;
    this.agent = agent;
    this.pulses = [];
    this.actions = [];
    this.loaded = false;
    this.loading = false;
    this.loadError = null;
    this.pending.clear();
    this.stopping.clear();
    this.framesDuringLoads.clear();
    this.watchFiles();
    if (this.watchers > 0) void this.load();
  }

  async load(): Promise<void> {
    const generation = this.generation;
    const seq = ++this.loadSeq;
    const framesSince: PulseFireFrame[] = [];
    this.framesDuringLoads.add(framesSince);
    this.loading = true;
    try {
      const agent = requireAgent(this.agent);
      const [pulses, actions] = await Promise.all([
        fetchScheduledPulses(agent),
        fetchScheduledActions(agent),
      ]);
      if (generation !== this.generation || seq < this.appliedSeq) return;
      this.appliedSeq = seq;
      this.pulses = framesSince.reduce(mergeFireFrame, pulses);
      this.actions = actions;
      this.forgetFinishedStops();
      this.loaded = true;
      this.loadError = null;
    } catch (err) {
      if (generation !== this.generation || seq < this.appliedSeq) return;
      this.loadError = userErrorMessage(err, { action: "Couldn't load the schedule." });
    } finally {
      this.framesDuringLoads.delete(framesSince);
      if (generation === this.generation) this.loading = false;
    }
  }

  /** Take a fire frame's run in flight, and its outcome when it has one, onto its pulse. */
  private applyFireFrame(frame: PulseFireFrame): void {
    for (const frames of this.framesDuringLoads) frames.push(frame);
    this.pulses = mergeFireFrame(this.pulses, frame);
    this.forgetFinishedStops();
  }

  /** Drop the stops whose run no pulse or action shows in flight any more. */
  private forgetFinishedStops(): void {
    if (this.stopping.size === 0) return;
    const live = [...this.pulses, ...this.actions].map((item) =>
      item.current_run === null ? null : stopTarget(item.current_run),
    );
    for (const target of [...this.stopping]) {
      if (!live.includes(target)) this.stopping.delete(target);
    }
  }

  /**
   * Stop the run in flight of the pulse or action named `name`: its fire, or
   * its session when no fire tracks it. The frames that follow show it end;
   * a failure is a toast.
   */
  async stopRun(kind: "pulse" | "action", name: string, run: ScheduledCurrentRun): Promise<void> {
    const target = stopTarget(run);
    if (target === null || this.stopping.has(target)) return;
    const generation = this.generation;
    this.stopping.add(target);
    try {
      const agent = requireAgent(this.agent);
      if (kind === "pulse" && run.fire_id !== null) {
        await stopPulseFire(agent, name, run.fire_id);
      } else {
        await stopSession(agent, target);
      }
    } catch (err) {
      if (generation !== this.generation) return;
      this.stopping.delete(target);
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't stop ${kind} "${name}".`,
          notFound: "It had already finished.",
        }),
      );
      // The run is over, so show where it stands now.
      if (err instanceof ApiError && err.status === 404) void this.load();
    }
  }

  private setEnabled(name: string, enabled: boolean): void {
    this.pulses = this.pulses.map((p) => (p.name === name ? { ...p, enabled } : p));
  }

  /** Turn a pulse on or off. The switch moves at once, and moves back if the change fails. */
  async toggleEnabled(pulse: PulseInfo): Promise<void> {
    if (this.pending.has(pulse.name)) return;
    const generation = this.generation;
    const next = !pulse.enabled;
    this.pending.add(pulse.name);
    this.setEnabled(pulse.name, next);
    try {
      await apiSetPulseEnabled(requireAgent(this.agent), pulse.name, next);
    } catch (err) {
      if (generation !== this.generation) return;
      this.setEnabled(pulse.name, !next);
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't ${next ? "resume" : "pause"} pulse "${pulse.name}".`,
        }),
      );
    } finally {
      if (generation === this.generation) this.pending.delete(pulse.name);
    }
  }

  async cancelAction(action: ActionInfo): Promise<void> {
    if (this.pending.has(action.id)) return;
    const generation = this.generation;
    this.pending.add(action.id);
    try {
      await apiCancelScheduledAction(requireAgent(this.agent), action.id);
      if (generation === this.generation) {
        this.actions = this.actions.filter((a) => a.id !== action.id);
      }
    } catch (err) {
      if (generation !== this.generation) return;
      notifications.surface(
        "error",
        userErrorMessage(err, { action: `Couldn't cancel scheduled action "${action.name}".` }),
      );
    } finally {
      if (generation === this.generation) this.pending.delete(action.id);
    }
  }
}

/**
 * `pulses` with a fire frame merged: its pulse's run in flight is the frame's
 * (`null` included), and its last outcome changes only when the frame has one.
 */
function mergeFireFrame(pulses: PulseInfo[], frame: PulseFireFrame): PulseInfo[] {
  return pulses.map((pulse) =>
    pulse.name === frame.pulse
      ? {
          ...pulse,
          current_run: frame.current_run,
          last_outcome: frame.last_outcome ?? pulse.last_outcome,
        }
      : pulse,
  );
}

export const scheduled = new ScheduledStore();
