import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { notifications } from "./notifications.svelte";
import { scheduled, type ScheduledSources } from "./scheduled.svelte";
import type {
  ActionInfo,
  PulseInfo,
  ScheduledCurrentRun,
  ScheduledRunOutcome,
  ServerMessage,
} from "./types";
import { WatchRegistry } from "./watch-registry";
import { normalizeWatchPrefix } from "./workspace-watch";

function pulse(name: string, overrides: Partial<PulseInfo> = {}): PulseInfo {
  return {
    name,
    enabled: true,
    schedule: "2h",
    active_hours: null,
    agent: null,
    model_tier: null,
    next_fire_at: "2026-03-14T14:00:00Z",
    last_outcome: null,
    current_run: null,
    problems: [],
    ...overrides,
  };
}

function action(id: string, name: string): ActionInfo {
  return {
    id,
    name,
    run_at: "2026-03-14T18:00:00Z",
    agent: null,
    model_tier: null,
    problem: null,
    current_run: null,
  };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** A stand-in for an agent's schedule routes, recording what was asked of it. */
interface Server {
  pulses: PulseInfo[];
  actions: ActionInfo[];
  /** Answer every request with a server fault. */
  failing: boolean;
  /** Each request as `METHOD path`. */
  requests: string[];
  /** The status a stop answers with. */
  stopStatus: number;
  /** Holds for the next pulses loads, in order: each waits for its hold to settle before it answers. */
  pulsesHolds: Promise<void>[];
}

let server: Server;
let registry: WatchRegistry;
/** Every watch set the registry sent, in order. */
let sent: string[][];
let frameListeners: Set<(msg: ServerMessage) => void>;
let sources: ScheduledSources;

function loads(agent = "atlas"): number {
  return server.requests.filter((r) => r === `GET /api/agents/${agent}/scheduled/pulses`).length;
}

function emit(msg: ServerMessage): void {
  registry.handleFrame(msg);
  for (const listener of frameListeners) listener(msg);
}

beforeEach(() => {
  server = {
    pulses: [pulse("inbox_check")],
    actions: [action("act-1", "weekly_digest")],
    failing: false,
    requests: [],
    stopStatus: 202,
    pulsesHolds: [],
  };
  vi.stubGlobal("fetch", async (input: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    server.requests.push(`${method} ${input}`);
    if (server.failing) return json({ error: "boom" }, 500);
    if (input.endsWith("/stop")) return json({ error: "atlas is stopped" }, server.stopStatus);
    if (input.endsWith("/scheduled/pulses")) {
      // Answer with the pulses as they were when asked, however long the hold.
      const answer = json(server.pulses);
      const waitFor = server.pulsesHolds.shift();
      if (waitFor) await waitFor;
      return answer;
    }
    if (input.endsWith("/scheduled/actions")) return json(server.actions);
    return json({});
  });
  sent = [];
  registry = new WatchRegistry({
    send: (prefixes) => sent.push(prefixes),
    normalize: normalizeWatchPrefix,
    refusal: (prefix) => `refused ${prefix}`,
  });
  frameListeners = new Set();
  sources = {
    onFrame: (listener) => {
      frameListeners.add(listener);
      return () => frameListeners.delete(listener);
    },
    watches: registry,
  };
  registry.bind("atlas");
  registry.connected();
  scheduled.reset("atlas");
});

afterEach(() => {
  scheduled.stopWatching();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the scheduled store's watch", () => {
  it("watches the two schedule files on the bound agent's socket while a view is open", () => {
    scheduled.startWatching(sources);
    expect(sent.at(-1)).toEqual(["HEARTBEAT.yml", "scheduled_actions.json"]);

    scheduled.stopWatching();
    expect(sent.at(-1)).toEqual([]);
    expect(frameListeners.size).toBe(0);
  });

  it("keeps watching until the last of two open views closes", () => {
    scheduled.startWatching(sources);
    scheduled.startWatching(sources);
    scheduled.stopWatching();
    expect(registry.prefixes).toEqual(["HEARTBEAT.yml", "scheduled_actions.json"]);
    scheduled.stopWatching();
    expect(registry.prefixes).toEqual([]);
  });

  it("reloads on a change to either file, and not on any other file", async () => {
    scheduled.startWatching(sources);
    emit({ type: "workspace_changed", changes: [{ path: "HEARTBEAT.yml", kind: "modified" }] });
    await expect.poll(() => loads()).toBe(1);
    emit({
      type: "workspace_changed",
      changes: [{ path: "scheduled_actions.json", kind: "modified" }],
    });
    await expect.poll(() => loads()).toBe(2);
    emit({ type: "workspace_changed", changes: [{ path: "notes/today.md", kind: "modified" }] });
    await Promise.resolve();
    expect(loads()).toBe(2);
  });

  it("reloads when the change feed lost track, and on a run's session frame", async () => {
    scheduled.startWatching(sources);
    emit({ type: "workspace_resync", reason: "overflow" });
    await expect.poll(() => loads()).toBe(1);
    emit({ type: "session_completed", address: "scheduled-1", run_id: "run-1" } as ServerMessage);
    await expect.poll(() => loads()).toBe(2);
  });

  it("moves its watch to the new agent on a switch, tied to that agent", async () => {
    scheduled.startWatching(sources);
    registry.bind("scout");
    scheduled.reset("scout");
    registry.connected();
    expect(sent.at(-1)).toEqual(["HEARTBEAT.yml", "scheduled_actions.json"]);
    await expect.poll(() => loads("scout")).toBe(1);

    emit({ type: "workspace_changed", changes: [{ path: "HEARTBEAT.yml", kind: "modified" }] });
    await expect.poll(() => loads("scout")).toBe(2);
    expect(loads("atlas")).toBe(0);

    // Bound back to atlas, the scout watch no longer applies.
    registry.bind("atlas");
    registry.connected();
    expect(registry.prefixes).toEqual([]);
  });
});

describe("loading the schedule", () => {
  it("keeps a failure in loadError, with no toast, until a load succeeds", async () => {
    const surface = vi.spyOn(notifications, "surface");
    server.failing = true;
    await scheduled.load();
    expect(scheduled.loaded).toBe(false);
    expect(scheduled.loadError).toMatch(/^Couldn't load the schedule\. /);
    expect(surface).not.toHaveBeenCalled();

    server.failing = false;
    await scheduled.load();
    expect(scheduled.loadError).toBeNull();
    expect(scheduled.loaded).toBe(true);
    expect(scheduled.pulses.map((p) => p.name)).toEqual(["inbox_check"]);
  });

  it("forgets an earlier agent's failure on a switch", async () => {
    server.failing = true;
    await scheduled.load();
    scheduled.reset("scout");
    expect(scheduled.loadError).toBeNull();
  });
});

describe("changing the schedule", () => {
  it("moves a pulse's switch at once and keeps it when the change lands", async () => {
    await scheduled.load();
    const toggling = scheduled.toggleEnabled(scheduled.pulses[0] ?? pulse("missing"));
    expect(scheduled.pulses[0]?.enabled).toBe(false);
    expect(scheduled.pending.has("inbox_check")).toBe(true);
    await toggling;
    expect(scheduled.pulses[0]?.enabled).toBe(false);
    expect(scheduled.pending.has("inbox_check")).toBe(false);
    expect(server.requests).toContain("PUT /api/agents/atlas/scheduled/pulses/inbox_check/enabled");
  });

  it("moves the switch back and says so when the change fails", async () => {
    await scheduled.load();
    const surface = vi.spyOn(notifications, "surface");
    server.failing = true;
    await scheduled.toggleEnabled(scheduled.pulses[0] ?? pulse("missing"));
    expect(scheduled.pulses[0]?.enabled).toBe(true);
    expect(scheduled.pending.size).toBe(0);
    expect(surface).toHaveBeenCalledWith(
      "error",
      expect.stringMatching(/^Couldn't pause pulse "inbox_check"\./),
    );
  });

  it("drops a cancelled action, and keeps one whose cancel fails", async () => {
    await scheduled.load();
    const surface = vi.spyOn(notifications, "surface");
    server.failing = true;
    await scheduled.cancelAction(scheduled.actions[0] ?? action("missing", "missing"));
    expect(scheduled.actions).toHaveLength(1);
    expect(surface).toHaveBeenCalledWith("error", expect.stringMatching(/^Couldn't cancel/));

    server.failing = false;
    await scheduled.cancelAction(scheduled.actions[0] ?? action("missing", "missing"));
    expect(scheduled.actions).toEqual([]);
    expect(server.requests).toContain("DELETE /api/agents/atlas/scheduled/actions/act-1");
  });
});

/** A fire of a pulse in flight, running its task scripts. */
function scriptRun(fireId: string): ScheduledCurrentRun {
  return {
    fire_id: fireId,
    phase: "script",
    started_at: "2026-03-14T12:00:00Z",
    address: null,
    run_id: null,
    overlap: null,
  };
}

function fireFrame(
  fireId: string,
  currentRun: ScheduledCurrentRun | null,
  lastOutcome: ScheduledRunOutcome | null = null,
): ServerMessage {
  return {
    type: "pulse_fire_changed",
    pulse: "inbox_check",
    fire_id: fireId,
    current_run: currentRun,
    last_outcome: lastOutcome,
  };
}

/** A hold for a pulses load, and the function that lets it answer. */
function hold(): { promise: Promise<void>; release: () => void } {
  let release = (): void => {};
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

const earlier: ScheduledRunOutcome = {
  status: "completed",
  at: "2026-03-14T10:00:00Z",
  error: null,
};
const failed: ScheduledRunOutcome = {
  status: "failed",
  at: "2026-03-14T12:01:00Z",
  error: "check_disk.sh exited with status 1",
};
const stopped: ScheduledRunOutcome = {
  status: "cancelled",
  at: "2026-03-14T12:01:00Z",
  error: null,
};

describe("pulse fire frames", () => {
  it("take the frame's run in flight at once, and its outcome only when it has one", async () => {
    server.pulses = [pulse("inbox_check", { last_outcome: earlier }), pulse("nightly_review")];
    await scheduled.load();
    scheduled.startWatching(sources);

    emit(fireFrame("fire-1", scriptRun("fire-1")));
    expect(scheduled.pulses[0]).toMatchObject({
      current_run: scriptRun("fire-1"),
      last_outcome: earlier,
    });
    expect(scheduled.pulses[1]?.current_run).toBeNull();
    // The fire moved the pulse's next run, so the schedule is read again.
    server.pulses = [
      pulse("inbox_check", { last_outcome: earlier, current_run: scriptRun("fire-1") }),
    ];
    await expect.poll(() => loads()).toBe(2);

    emit(fireFrame("fire-1", null, failed));
    expect(scheduled.pulses[0]).toMatchObject({ current_run: null, last_outcome: failed });
  });

  it("keep a frame that lands during a load over that load's older answer", async () => {
    await scheduled.load();
    scheduled.startWatching(sources);
    const stale = hold();
    const fresh = hold();
    server.pulsesHolds.push(stale.promise, fresh.promise);
    const staleLoad = scheduled.load();
    await expect.poll(() => loads()).toBe(2);

    // The fire starts after the server answered that load.
    server.pulses = [pulse("inbox_check", { current_run: scriptRun("fire-1") })];
    emit(fireFrame("fire-1", scriptRun("fire-1")));
    await expect.poll(() => loads()).toBe(3);

    stale.release();
    await staleLoad;
    expect(scheduled.pulses[0]?.current_run).toEqual(scriptRun("fire-1"));
    fresh.release();
  });
});

describe("stopping a run", () => {
  it("stops a fire through its pulse, and shows it stopping until the fire is gone", async () => {
    server.pulses = [pulse("inbox_check", { current_run: scriptRun("fire-1") })];
    await scheduled.load();
    scheduled.startWatching(sources);

    await scheduled.stopRun("pulse", "inbox_check", scriptRun("fire-1"));
    expect(server.requests).toContain(
      "POST /api/agents/atlas/scheduled/pulses/inbox_check/fires/fire-1/stop",
    );
    expect(scheduled.stopping.has("fire-1")).toBe(true);

    server.pulses = [pulse("inbox_check", { last_outcome: stopped })];
    emit(fireFrame("fire-1", null, stopped));
    expect(scheduled.stopping.has("fire-1")).toBe(false);
    expect(scheduled.pulses[0]?.last_outcome).toEqual(stopped);
  });

  it("says a fire had already finished, and reads where its pulse stands", async () => {
    server.pulses = [pulse("inbox_check", { current_run: scriptRun("fire-1") })];
    await scheduled.load();
    const surface = vi.spyOn(notifications, "surface");
    server.stopStatus = 404;
    server.pulses = [pulse("inbox_check", { last_outcome: failed })];

    await scheduled.stopRun("pulse", "inbox_check", scriptRun("fire-1"));
    expect(surface).toHaveBeenCalledWith(
      "error",
      'Couldn\'t stop pulse "inbox_check". It had already finished.',
    );
    expect(scheduled.stopping.size).toBe(0);
    await expect.poll(() => scheduled.pulses[0]?.last_outcome).toEqual(failed);
    expect(scheduled.pulses[0]?.current_run).toBeNull();
  });

  it("says why when the agent can't stop it", async () => {
    server.pulses = [pulse("inbox_check", { current_run: scriptRun("fire-1") })];
    await scheduled.load();
    const surface = vi.spyOn(notifications, "surface");
    server.stopStatus = 409;

    await scheduled.stopRun("pulse", "inbox_check", scriptRun("fire-1"));
    expect(surface).toHaveBeenCalledWith(
      "error",
      'Couldn\'t stop pulse "inbox_check". Atlas is stopped.',
    );
    expect(scheduled.stopping.size).toBe(0);
  });

  it("stops a run no fire tracks through its session", async () => {
    const run: ScheduledCurrentRun = {
      fire_id: null,
      phase: "session",
      started_at: "2026-03-14T12:00:00Z",
      address: "scheduled-1",
      run_id: "run-1",
      overlap: null,
    };
    server.actions = [{ ...action("act-1", "weekly_digest"), current_run: run }];
    await scheduled.load();

    await scheduled.stopRun("action", "weekly_digest", run);
    expect(server.requests).toContain("POST /api/agents/atlas/sessions/scheduled-1/stop");
    expect(scheduled.stopping.has("scheduled-1")).toBe(true);

    // The session's frames read the schedule again, which no longer shows the run.
    server.actions = [action("act-1", "weekly_digest")];
    await scheduled.load();
    expect(scheduled.stopping.size).toBe(0);
  });
});
