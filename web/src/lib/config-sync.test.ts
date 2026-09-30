import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import {
  agentConfigFile,
  ConfigCoordinator,
  configCoordinator,
  HUB_CONFIG_FILE,
  type ConfigFile,
  type ConfigIo,
} from "./config-coordinator";
import { followConfigChanges, startConfigSync, type ConfigSyncSources } from "./config-sync";
import { hub } from "./hub.svelte";
import type { HubServerMessage } from "./hub-types";
import { setViewedAgent } from "./viewed-agent";
import type { WatchHandler, WatchOwner, WatchOwnerOptions } from "./watch-registry";
import { ws } from "./ws.svelte";
import type { WorkspaceChange } from "./types";

const modified = (...paths: string[]): WorkspaceChange[] =>
  paths.map((path) => ({ path, kind: "modified" as const }));

/** Let every queued promise callback run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

// ── Sources that are driven by hand ──────────────────────────────────

interface FakeOwner {
  handler: WatchHandler;
  options: WatchOwnerOptions;
  prefixes: readonly string[];
  released: boolean;
}

function fakeSources(boundAgent: string | null = null): {
  sources: ConfigSyncSources;
  owners: FakeOwner[];
  /** The agent the watch registry applies to: owners tied to another don't hear. */
  bind: (agent: string | null) => void;
  hubFrame: (frame: HubServerMessage) => void;
  listening: () => { agent: number; hub: number };
} {
  const owners: FakeOwner[] = [];
  let bound = boundAgent;
  const agentListeners = new Set<(agent: string | null) => void>();
  const hubListeners = new Set<(frame: HubServerMessage) => void>();
  return {
    owners,
    sources: {
      watches: {
        register: (handler, options) => {
          const owner: FakeOwner = { handler, options, prefixes: [], released: false };
          owners.push(owner);
          return {
            get prefixes() {
              return owner.prefixes;
            },
            set: (prefixes) => {
              owner.prefixes = prefixes;
            },
            release: () => {
              owner.released = true;
            },
          } satisfies WatchOwner;
        },
      },
      boundAgent: () => bound,
      onAgentChange: (listener) => {
        agentListeners.add(listener);
        return () => agentListeners.delete(listener);
      },
      onHubFrame: (listener) => {
        hubListeners.add(listener);
        return () => hubListeners.delete(listener);
      },
    },
    bind: (agent) => {
      bound = agent;
      for (const listener of agentListeners) listener(agent);
    },
    hubFrame: (frame) => {
      for (const listener of hubListeners) listener(frame);
    },
    listening: () => ({ agent: agentListeners.size, hub: hubListeners.size }),
  };
}

/** A server whose files read as `text` until a test changes them. */
function serverOf(text: string): { io: ConfigIo; files: Map<string, string> } {
  const files = new Map<string, string>();
  return {
    files,
    io: {
      read: (file: ConfigFile) => Promise.resolve(files.get(JSON.stringify(file)) ?? text),
      patch: () => Promise.resolve({ valid: true }),
      put: () => Promise.resolve({ valid: true }),
      restore: () => Promise.resolve({ checkpoint_id: "x", restored_paths: [] }),
      undo: () => Promise.resolve({ checkpoint_id: "x", reverted_paths: [], skipped_paths: [] }),
    },
  };
}

function hearEach(
  coordinator: ConfigCoordinator,
  files: Record<string, ConfigFile>,
): Record<string, string[]> {
  const heard: Record<string, string[]> = {};
  for (const [name, file] of Object.entries(files)) {
    heard[name] = [];
    coordinator.subscribe(file, (change) => heard[name]?.push(change.cause));
  }
  return heard;
}

const scoutFiles = {
  config: agentConfigFile("scout", "config"),
  providers: agentConfigFile("scout", "providers"),
  mcp: agentConfigFile("scout", "mcp"),
};

describe("following the bound agent's config folder", () => {
  it("watches `config` for the bound agent, tied to that agent", () => {
    const { sources, owners } = fakeSources("scout");
    followConfigChanges(new ConfigCoordinator(serverOf("").io), sources);

    expect(owners).toHaveLength(1);
    expect(owners[0]?.options).toEqual({ agent: "scout" });
    expect(owners[0]?.prefixes).toEqual(["config"]);
  });

  it("follows the viewed agent, releasing the watch it had", () => {
    const { sources, owners, bind } = fakeSources(null);
    followConfigChanges(new ConfigCoordinator(serverOf("").io), sources);
    expect(owners).toHaveLength(0);

    bind("scout");
    bind("atlas");
    bind(null);

    expect(owners.map((o) => [o.options.agent, o.released])).toEqual([
      ["scout", true],
      ["atlas", true],
    ]);
  });

  it("tells subscribers about the config file that changed, and no other", async () => {
    const { sources, owners } = fakeSources("scout");
    const server = serverOf("a = 1\n");
    const coordinator = new ConfigCoordinator(server.io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, scoutFiles);

    owners[0]?.handler.changed(modified("config/providers.toml"));
    await flush();

    expect(heard).toEqual({ config: [], providers: ["external"], mcp: [] });
  });

  it("tells subscribers about every config file when the folder itself changed", async () => {
    const { sources, owners } = fakeSources("scout");
    const coordinator = new ConfigCoordinator(serverOf("a = 1\n").io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, scoutFiles);

    owners[0]?.handler.changed([{ path: "config", kind: "removed" }]);
    await flush();

    expect(heard).toEqual({ config: ["external"], providers: ["external"], mcp: ["external"] });
  });

  it("ignores other files in the folder", async () => {
    const { sources, owners } = fakeSources("scout");
    const coordinator = new ConfigCoordinator(serverOf("a = 1\n").io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, scoutFiles);

    owners[0]?.handler.changed(
      modified("config/providers.toml.tmp", "config/config.last-known-good.toml"),
    );
    await flush();

    expect(heard).toEqual({ config: [], providers: [], mcp: [] });
  });

  it("tells subscribers about all three files when the feed loses track of changes", async () => {
    const { sources, owners } = fakeSources("scout");
    const coordinator = new ConfigCoordinator(serverOf("a = 1\n").io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, scoutFiles);

    owners[0]?.handler.resync?.("overflow");
    await flush();

    expect(heard).toEqual({ config: ["external"], providers: ["external"], mcp: ["external"] });
  });

  it("stays quiet about a change it already knows of", async () => {
    const { sources, owners } = fakeSources("scout");
    const coordinator = new ConfigCoordinator(serverOf("a = 1\n").io);
    followConfigChanges(coordinator, sources);
    await coordinator.reload(scoutFiles.providers);
    const heard = hearEach(coordinator, scoutFiles);

    owners[0]?.handler.changed(modified("config/providers.toml"));
    await flush();

    expect(heard.providers).toEqual([]);
  });
});

describe("following the hub's config reloads", () => {
  const reloaded = (ok: boolean, changed: boolean): HubServerMessage => ({
    type: "hub_config_reloaded",
    ok,
    changed,
    message: null,
  });

  it("tells subscribers when a reload applied a change", async () => {
    const { sources, hubFrame } = fakeSources();
    const coordinator = new ConfigCoordinator(serverOf('timezone = "UTC"\n').io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, { hub: HUB_CONFIG_FILE });

    hubFrame(reloaded(true, true));
    await flush();

    expect(heard.hub).toEqual(["external"]);
  });

  it("tells them when a reload couldn't load the file, which changed on disk all the same", async () => {
    const { sources, hubFrame } = fakeSources();
    const coordinator = new ConfigCoordinator(serverOf("broken").io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, { hub: HUB_CONFIG_FILE });

    hubFrame(reloaded(false, false));
    await flush();

    expect(heard.hub).toEqual(["external"]);
  });

  it("stays quiet when a reload found nothing to apply", async () => {
    const { sources, hubFrame } = fakeSources();
    const coordinator = new ConfigCoordinator(serverOf("").io);
    followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, { hub: HUB_CONFIG_FILE });

    hubFrame(reloaded(true, false));
    hubFrame({ type: "hub_boot", boot_id: "b" });
    await flush();

    expect(heard.hub).toEqual([]);
  });
});

describe("stopping", () => {
  it("releases the watch and stops listening", async () => {
    const { sources, owners, hubFrame, bind, listening } = fakeSources("scout");
    const coordinator = new ConfigCoordinator(serverOf("a = 1\n").io);
    const stop = followConfigChanges(coordinator, sources);
    const heard = hearEach(coordinator, { hub: HUB_CONFIG_FILE });

    stop();
    hubFrame({ type: "hub_config_reloaded", ok: true, changed: true, message: null });
    bind("atlas");
    await flush();

    expect(owners.map((o) => o.released)).toEqual([true]);
    expect(listening()).toEqual({ agent: 0, hub: 0 });
    expect(heard.hub).toEqual([]);
  });
});

// ── The app's sockets ─────────────────────────────────────────────────

describe("the app's coordinator, through the real sockets", () => {
  let stop: () => void;

  beforeEach(() => {
    FakeWebSocket.install();
    vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
    // Config files answer; everything else the agent socket loads never does.
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const url = typeof input === "string" ? input : "";
        if (url.endsWith("/raw")) return Promise.resolve(new Response("a = 1\n"));
        return new Promise<Response>(() => {});
      }),
    );
    stop = startConfigSync();
  });

  afterEach(() => {
    stop();
    setViewedAgent(null);
    vi.unstubAllGlobals();
  });

  it("hears a workspace change under the bound agent's config folder", async () => {
    setViewedAgent("river");
    FakeWebSocket.last.simulateOpen();
    expect(
      FakeWebSocket.last
        .sentFrames()
        .filter((f) => (f as { type: string }).type === "watch_workspace"),
    ).toEqual([{ type: "watch_workspace", prefixes: ["config"] }]);
    const heard = hearEach(configCoordinator, {
      providers: agentConfigFile("river", "providers"),
      mcp: agentConfigFile("river", "mcp"),
    });

    ws.watches.handleFrame({
      type: "workspace_changed",
      changes: modified("config/providers.toml", "notes.md"),
    });
    await vi.waitFor(() => {
      expect(heard.providers).toEqual(["external"]);
    });

    expect(heard.mcp).toEqual([]);
  });

  it("does not hear a change once another agent is bound", async () => {
    setViewedAgent("river");
    setViewedAgent("lake");
    FakeWebSocket.last.simulateOpen();
    const heard = hearEach(configCoordinator, {
      river: agentConfigFile("river", "providers"),
      lake: agentConfigFile("lake", "providers"),
    });

    ws.watches.handleFrame({
      type: "workspace_changed",
      changes: modified("config/providers.toml"),
    });
    await vi.waitFor(() => {
      expect(heard.lake).toEqual(["external"]);
    });

    expect(heard.river).toEqual([]);
  });

  it("hears the hub's config reloading", async () => {
    const heard = hearEach(configCoordinator, { hub: HUB_CONFIG_FILE });

    hub.handleFrame({ type: "hub_config_reloaded", ok: true, changed: true, message: null });
    await vi.waitFor(() => {
      expect(heard.hub).toEqual(["external"]);
    });
  });
});
