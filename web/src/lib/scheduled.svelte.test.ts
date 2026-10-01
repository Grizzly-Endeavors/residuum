import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { notifications } from "./notifications.svelte";
import { scheduled, type ScheduledSources } from "./scheduled.svelte";
import type { ActionInfo, PulseInfo, ServerMessage } from "./types";
import { WatchRegistry } from "./watch-registry";
import { normalizeWatchPrefix } from "./workspace-watch";

function pulse(name: string, overrides: Partial<PulseInfo> = {}): PulseInfo {
  return {
    name,
    enabled: true,
    schedule: "2h",
    active_hours: null,
    agent: null,
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
  };
  vi.stubGlobal("fetch", (input: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    server.requests.push(`${method} ${input}`);
    if (server.failing) return Promise.resolve(json({ error: "boom" }, 500));
    if (input.endsWith("/scheduled/pulses")) return Promise.resolve(json(server.pulses));
    if (input.endsWith("/scheduled/actions")) return Promise.resolve(json(server.actions));
    return Promise.resolve(json({}));
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
