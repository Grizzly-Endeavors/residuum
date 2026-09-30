// ── Scheduled view state (Svelte 5 runes) ────────────────────────────
//
// Pulses and scheduled actions, for the Scheduled view. Loaded over REST
// (there's no dedicated WebSocket feed for pulse/action definitions) and
// refetched on signals the app already receives: a `workspace_changed`
// frame naming HEARTBEAT.yml or scheduled_actions.json (a pulse/action was
// added, edited, or removed), and any `scheduled`-category session frame
// (a pulse or action fired, or finished) — never on a bare poll timer.

import {
  fetchScheduledPulses,
  fetchScheduledActions,
  setPulseEnabled as apiSetPulseEnabled,
  cancelScheduledAction as apiCancelScheduledAction,
} from "./api";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";
import { requireAgent } from "./paths";
import type { ActionInfo, PulseInfo, ServerMessage } from "./types";

const WATCHED_PATHS = new Set(["HEARTBEAT.yml", "scheduled_actions.json"]);

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

class ScheduledStore {
  pulses = $state<PulseInfo[]>([]);
  actions = $state<ActionInfo[]>([]);
  loading = $state(false);
  loaded = $state(false);
  /** Names/ids currently being toggled or cancelled, to disable their controls. */
  pending = $state(new Set<string>());

  private unsubscribeFrame: (() => void) | null = null;
  private watchers = 0;
  /** Bumped on a reset, so a load begun for the previous agent can tell it is stale. */
  private generation = 0;
  /** The agent whose pulses and actions these are, `null` before one is bound. */
  private agent: string | null = null;

  /**
   * Start watching for refetch signals. Call once per mounted view; pairs
   * with `stopWatching()`. Reference-counted so more than one open view
   * doesn't double-subscribe or tear down the other's subscription.
   */
  startWatching(onFrame: (listener: (msg: ServerMessage) => void) => () => void): void {
    this.watchers++;
    if (this.unsubscribeFrame) return;
    this.unsubscribeFrame = onFrame((msg) => {
      if (msg.type === "workspace_changed" && msg.changes.some((c) => WATCHED_PATHS.has(c.path))) {
        void this.load();
        return;
      }
      if (isScheduledSessionFrame(msg)) void this.load();
    });
  }

  stopWatching(): void {
    this.watchers = Math.max(0, this.watchers - 1);
    if (this.watchers === 0 && this.unsubscribeFrame) {
      this.unsubscribeFrame();
      this.unsubscribeFrame = null;
    }
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
    this.pending = new Set();
    if (this.watchers > 0) void this.load();
  }

  async load(): Promise<void> {
    const generation = this.generation;
    this.loading = true;
    try {
      const agent = requireAgent(this.agent);
      const [pulses, actions] = await Promise.all([
        fetchScheduledPulses(agent),
        fetchScheduledActions(agent),
      ]);
      if (generation !== this.generation) return;
      this.pulses = pulses;
      this.actions = actions;
      this.loaded = true;
    } catch (err) {
      if (generation !== this.generation) return;
      notifications.surface(
        "error",
        userErrorMessage(err, { action: "Couldn't load the Scheduled view." }),
      );
    } finally {
      if (generation === this.generation) this.loading = false;
    }
  }

  async toggleEnabled(pulse: PulseInfo): Promise<void> {
    if (this.pending.has(pulse.name)) return;
    this.pending.add(pulse.name);
    this.pending = new Set(this.pending);
    const next = !pulse.enabled;
    try {
      await apiSetPulseEnabled(requireAgent(this.agent), pulse.name, next);
      const index = this.pulses.findIndex((p) => p.name === pulse.name);
      const current = this.pulses[index];
      if (current) {
        this.pulses[index] = { ...current, enabled: next };
      }
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't ${next ? "enable" : "disable"} pulse "${pulse.name}".`,
        }),
      );
    } finally {
      this.pending.delete(pulse.name);
      this.pending = new Set(this.pending);
    }
  }

  async cancelAction(action: ActionInfo): Promise<void> {
    if (this.pending.has(action.id)) return;
    this.pending.add(action.id);
    this.pending = new Set(this.pending);
    try {
      await apiCancelScheduledAction(requireAgent(this.agent), action.id);
      this.actions = this.actions.filter((a) => a.id !== action.id);
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, { action: `Couldn't cancel scheduled action "${action.name}".` }),
      );
    } finally {
      this.pending.delete(action.id);
      this.pending = new Set(this.pending);
    }
  }
}

export const scheduled = new ScheduledStore();
