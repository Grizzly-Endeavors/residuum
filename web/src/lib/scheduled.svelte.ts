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

const WATCHED_FILES = ["HEARTBEAT.yml", "scheduled_actions.json"];

export interface ScheduledSources {
  onFrame: (listener: (msg: ServerMessage) => void) => () => void;
  watches: { register: (handler: WatchHandler, options: WatchOwnerOptions) => WatchOwner };
}

function isScheduledSessionFrame(msg: ServerMessage): boolean {
  if (msg.type === "session_started") return msg.session.category === "scheduled";
  return msg.type === "session_state_changed" || msg.type === "session_completed";
}

type PulseFireFrame = Extract<ServerMessage, { type: "pulse_fire_changed" }>;

export function stopTarget(run: ScheduledCurrentRun): string | null {
  return run.fire_id ?? run.address;
}

class ScheduledStore {
  pulses = $state<PulseInfo[]>([]);
  actions = $state<ActionInfo[]>([]);
  loading = $state(false);
  loaded = $state(false);
  loadError = $state<string | null>(null);
  readonly pending = new SvelteSet<string>();
  readonly stopping = new SvelteSet<string>();

  private sources: ScheduledSources | null = null;
  private unsubscribeFrame: (() => void) | null = null;
  private fileWatch: WatchOwner | null = null;
  private watchers = 0;
  private generation = 0;
  private agent: string | null = null;
  private readonly framesDuringLoads = new Set<PulseFireFrame[]>();
  private loadSeq = 0;
  private appliedSeq = 0;

  startWatching(sources: ScheduledSources): void {
    this.watchers++;
    if (this.sources) return;
    this.sources = sources;
    this.unsubscribeFrame = sources.onFrame((msg) => {
      if (msg.type === "pulse_fire_changed") {
        this.applyFireFrame(msg);
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

  private watchFiles(): void {
    this.fileWatch?.release();
    this.fileWatch = null;
    if (this.sources === null || this.agent === null) return;
    const reload = (): void => void this.load();
    this.fileWatch = this.sources.watches.register(
      { changed: reload, resync: reload },
      { agent: this.agent },
    );
    this.fileWatch.set(WATCHED_FILES);
  }

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

  private applyFireFrame(frame: PulseFireFrame): void {
    for (const frames of this.framesDuringLoads) frames.push(frame);
    this.pulses = mergeFireFrame(this.pulses, frame);
    this.forgetFinishedStops();
  }

  private forgetFinishedStops(): void {
    if (this.stopping.size === 0) return;
    const live = [...this.pulses, ...this.actions].map((item) =>
      item.current_run === null ? null : stopTarget(item.current_run),
    );
    for (const target of [...this.stopping]) {
      if (!live.includes(target)) this.stopping.delete(target);
    }
  }

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
      if (err instanceof ApiError && err.status === 404) void this.load();
    }
  }

  private setEnabled(name: string, enabled: boolean): void {
    this.pulses = this.pulses.map((p) => (p.name === name ? { ...p, enabled } : p));
  }

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
