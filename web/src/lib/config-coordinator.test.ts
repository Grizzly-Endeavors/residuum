import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentConfigFile,
  checkpointLocationOf,
  ConfigCoordinator,
  configFileKey,
  HUB_CONFIG_FILE,
  type ConfigChange,
  type ConfigChooser,
  type ConfigFile,
  type ConfigIo,
} from "./config-coordinator";
import { applyPatch } from "../test/apply-patch";
import type { RestoreOutcome, UndoOutcome } from "./types";

// ── A fake server ─────────────────────────────────────────────────────

function patchText(file: ConfigFile, text: string, diff: Record<string, unknown>): string {
  return applyPatch(text, diff, file.kind === "agent" && file.name === "mcp" ? "json" : "toml");
}

interface FakeServer {
  io: ConfigIo;
  /** What each file holds, by `configFileKey`. */
  disk: Map<string, string>;
  /** Every request, in order: `read providers`, `patch providers`, and so on. */
  log: string[];
  patch: ReturnType<typeof vi.fn<ConfigIo["patch"]>>;
  put: ReturnType<typeof vi.fn<ConfigIo["put"]>>;
  restore: ReturnType<typeof vi.fn<ConfigIo["restore"]>>;
  undo: ReturnType<typeof vi.fn<ConfigIo["undo"]>>;
  /** Change a file the way something outside the page would. */
  outside: (file: ConfigFile, text: string) => void;
}

const label = (file: ConfigFile): string => (file.kind === "hub" ? "hub" : file.name);

function fakeServer(initial: [ConfigFile, string][] = []): FakeServer {
  const disk = new Map(initial.map(([file, text]) => [configFileKey(file), text]));
  const log: string[] = [];
  const read = vi.fn<ConfigIo["read"]>(async (file) => {
    log.push(`read ${label(file)}`);
    await Promise.resolve();
    return disk.get(configFileKey(file)) ?? "";
  });
  const patch = vi.fn<ConfigIo["patch"]>(async (file, diff) => {
    log.push(`patch ${label(file)}`);
    await Promise.resolve();
    disk.set(configFileKey(file), patchText(file, disk.get(configFileKey(file)) ?? "", diff));
    return { valid: true, checkpoint_id: "cp" };
  });
  const put = vi.fn<ConfigIo["put"]>(async (file, text) => {
    log.push(`put ${label(file)}`);
    await Promise.resolve();
    disk.set(configFileKey(file), text);
    return { valid: true };
  });
  const restore = vi.fn<ConfigIo["restore"]>(() =>
    Promise.resolve({ checkpoint_id: "new", restored_paths: [] } satisfies RestoreOutcome),
  );
  const undo = vi.fn<ConfigIo["undo"]>(() =>
    Promise.resolve({
      checkpoint_id: "new",
      reverted_paths: [],
      skipped_paths: [],
    } satisfies UndoOutcome),
  );
  return {
    io: { read, patch, put, restore, undo },
    disk,
    log,
    patch,
    put,
    restore,
    undo,
    outside: (file, text) => {
      disk.set(configFileKey(file), text);
    },
  };
}

const providers = agentConfigFile("scout", "providers");
const config = agentConfigFile("scout", "config");
const mcp = agentConfigFile("scout", "mcp");

const keepMine: ConfigChooser = () => Promise.resolve("keep-mine");

/** Record what a file's subscribers hear. */
function hear(
  coordinator: ConfigCoordinator,
  file: ConfigFile,
): { changes: ConfigChange[]; causes: () => string[] } {
  const changes: ConfigChange[] = [];
  coordinator.subscribe(file, (change) => changes.push(change));
  return { changes, causes: () => changes.map((c) => c.cause) };
}

function deferred(): { promise: Promise<void>; release: () => void } {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

/** Let every queued promise callback run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

let raised: unknown[];

beforeEach(() => {
  // A failing subscriber is raised as an uncaught error from a microtask;
  // catch it so a test can look at it.
  raised = [];
  vi.stubGlobal("queueMicrotask", (callback: () => void) => {
    try {
      callback();
    } catch (err) {
      raised.push(err);
    }
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

// ── Serialization ─────────────────────────────────────────────────────

describe("serializing writes", () => {
  it("runs one write to a file at a time, each reading what the last wrote", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const gate = deferred();
    server.patch.mockImplementationOnce(async (file, diff) => {
      server.log.push("patch providers (slow)");
      await gate.promise;
      server.disk.set(configFileKey(file), patchText(file, "a = 1\n", diff));
      return { valid: true };
    });
    const seen: string[] = [];

    const first = coordinator.edit(providers, (raw) => {
      seen.push(raw);
      return { a: 2 };
    });
    const second = coordinator.edit(providers, (raw) => {
      seen.push(raw);
      return { a: 3 };
    });
    await flush();

    // The second has not even read the file: the first still holds it.
    expect(server.log).toEqual(["read providers", "patch providers (slow)"]);

    gate.release();
    await Promise.all([first, second]);

    expect(seen).toEqual(["a = 1\n", "a = 2\n"]);
    expect(server.disk.get(configFileKey(providers))).toBe("a = 3\n");
  });

  it("lets writes to different files go on together", async () => {
    const server = fakeServer([
      [providers, "a = 1\n"],
      [config, "b = 1\n"],
    ]);
    const coordinator = new ConfigCoordinator(server.io);
    const gate = deferred();
    server.patch.mockImplementationOnce(async () => {
      await gate.promise;
      return { valid: true };
    });

    const slow = coordinator.edit(providers, () => ({ a: 2 }));
    await flush();
    const other = await coordinator.edit(config, () => ({ b: 2 }));

    expect(other.written).toBe(true);
    gate.release();
    await slow;
  });

  it("keeps the next write going after one fails", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.patch.mockRejectedValueOnce(new Error("disk full"));

    const failed = coordinator.edit(providers, () => ({ a: 2 }));
    const after = coordinator.edit(providers, () => ({ a: 3 }));

    await expect(failed).rejects.toThrow("disk full");
    await expect(after).resolves.toMatchObject({ written: true });
    expect(server.disk.get(configFileKey(providers))).toBe("a = 3\n");
  });

  it("holds no lock while the caller is asked to choose", async () => {
    const server = fakeServer([[providers, "a = 1\nb = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const answer = deferred();
    const choose = vi.fn<ConfigChooser>(async () => {
      await answer.promise;
      return "keep-mine";
    });

    const saving = coordinator.save(providers, {
      baseline: "a = 1\nb = 1\n",
      edit: { patch: { b: 3 } },
      choose,
    });
    await vi.waitFor(() => {
      expect(choose).toHaveBeenCalledTimes(1);
    });

    // The composer can still write the file while the user decides.
    const other = await coordinator.edit(providers, () => ({ a: 9 }));
    expect(other.written).toBe(true);

    answer.release();
    await saving;
    expect(server.disk.get(configFileKey(providers))).toBe("a = 9\nb = 3\n");
  });
});

// ── The re-read before a save ─────────────────────────────────────────

describe("saving against a file that changed under the caller", () => {
  const baseline = "a = 1\nb = 1\n";

  it("goes ahead when the change is to other keys, and keeps it", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { a: 5 } },
      choose,
    });

    expect(choose).not.toHaveBeenCalled();
    expect(outcome).toMatchObject({ kind: "saved", written: true, raw: "a = 5\nb = 2\n" });
    expect(server.disk.get(configFileKey(providers))).toBe("a = 5\nb = 2\n");
  });

  it("goes ahead when only a comment or the key order changed", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "# note\nb = 1\na = 1\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, { baseline, edit: { patch: { a: 5 } }, choose });

    expect(choose).not.toHaveBeenCalled();
    expect(server.patch).toHaveBeenCalledTimes(1);
  });

  it("asks when the change is to a key the save sets, and names the key", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, { baseline, edit: { patch: { b: 5 } }, choose });

    expect(choose).toHaveBeenCalledTimes(1);
    expect(choose).toHaveBeenCalledWith({
      file: providers,
      keys: ["b"],
      disk: "a = 1\nb = 2\n",
    });
  });

  it("writes the caller's changes when it keeps them", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { b: 5 } },
      choose: keepMine,
    });

    expect(outcome).toMatchObject({ kind: "saved", written: true });
    expect(server.disk.get(configFileKey(providers))).toBe("a = 1\nb = 5\n");
  });

  it("writes nothing and hands back the disk's text when the caller takes it", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const heard = hear(coordinator, providers);

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { b: 5 } },
      choose: () => Promise.resolve("use-disk"),
      source: Symbol("page"),
    });

    expect(outcome).toEqual({ kind: "used-disk", raw: "a = 1\nb = 2\n" });
    expect(server.patch).not.toHaveBeenCalled();
    // Every view is told to show the disk's text.
    expect(heard.causes()).toEqual(["reload"]);
  });

  it("treats a removed table as overlapping a change inside it", async () => {
    const server = fakeServer([[providers, "[providers.acme]\napi_key = 'x'\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "[providers.acme]\napi_key = 'y'\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, {
      baseline: "[providers.acme]\napi_key = 'x'\n",
      edit: { patch: { providers: { acme: null } } },
      choose,
    });

    expect(choose).toHaveBeenCalledWith(
      expect.objectContaining({ keys: ["providers.acme.api_key"] }),
    );
  });

  it("treats an inline model assignment as one key", async () => {
    const before = "[models]\nmain = { model = 'a/x', thinking = 'low' }\n";
    const server = fakeServer([[providers, before]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "[models]\nmain = { model = 'a/x', thinking = 'high' }\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, {
      baseline: before,
      edit: { patch: { models: { main: { $inline: { model: "a/y" } } } } },
      choose,
    });

    expect(choose).toHaveBeenCalledWith(
      expect.objectContaining({ keys: ["models.main.thinking"] }),
    );
  });

  it("compares mcp.json by its keys too", async () => {
    const before = '{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}';
    const server = fakeServer([[mcp, before]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(mcp, '{"mcpServers":{"a":{"command":"x"},"b":{"command":"z"}}}');
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(mcp, {
      baseline: before,
      edit: { patch: { mcpServers: { a: { command: "w" } } } },
      choose,
    });
    expect(choose).not.toHaveBeenCalled();

    server.outside(mcp, '{"mcpServers":{"a":{"command":"v"},"b":{"command":"z"}}}');
    await coordinator.save(mcp, {
      baseline: before,
      edit: { patch: { mcpServers: { a: null } } },
      choose,
    });
    expect(choose).toHaveBeenCalledWith(
      expect.objectContaining({ keys: ["mcpServers.a.command"] }),
    );
  });

  it("asks before a raw save over any change made elsewhere", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, { baseline, edit: { text: "a = 7\nb = 1\n" }, choose });

    expect(choose).toHaveBeenCalledWith(expect.objectContaining({ keys: ["b"] }));
    expect(server.put).toHaveBeenCalledTimes(1);
  });

  it("does not ask when the file is as the caller loaded it", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, { baseline, edit: { text: "a = 7\n" }, choose });

    expect(choose).not.toHaveBeenCalled();
    expect(server.disk.get(configFileKey(providers))).toBe("a = 7\n");
  });

  it("asks again only about a change made after the one the caller decided against", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const answers = [deferred(), deferred()];
    let asked = 0;
    const choose = vi.fn<ConfigChooser>(async () => {
      await answers[asked++]?.promise;
      return "keep-mine";
    });

    const saving = coordinator.save(providers, { baseline, edit: { patch: { b: 5 } }, choose });
    await flush();
    expect(choose).toHaveBeenCalledTimes(1);

    // While the user decides, something changes the same key again.
    server.outside(providers, "a = 1\nb = 3\n");
    answers[0]?.release();
    await flush();
    expect(choose).toHaveBeenCalledTimes(2);
    expect(choose).toHaveBeenLastCalledWith(
      expect.objectContaining({ keys: ["b"], disk: "a = 1\nb = 3\n" }),
    );

    answers[1]?.release();
    await saving;
    expect(server.disk.get(configFileKey(providers))).toBe("a = 1\nb = 5\n");
  });

  it("does not ask again when the same text is still on disk", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = 1\nb = 2\n");
    const choose = vi.fn<ConfigChooser>(keepMine);

    await coordinator.save(providers, { baseline, edit: { patch: { b: 5 } }, choose });

    expect(choose).toHaveBeenCalledTimes(1);
  });

  it("goes ahead when the file on disk can't be read as TOML, leaving the server to report it", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.outside(providers, "a = = broken");
    const choose = vi.fn<ConfigChooser>(keepMine);
    server.patch.mockResolvedValueOnce({ valid: false, error: "providers.toml is not valid TOML" });

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { a: 2 } },
      choose,
    });

    expect(choose).not.toHaveBeenCalled();
    expect(outcome).toMatchObject({ kind: "saved", written: false });
  });

  it("sends nothing for an empty patch", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: {} },
      choose: keepMine,
    });

    expect(outcome).toMatchObject({ kind: "saved", written: false, raw: baseline });
    expect(server.log).toEqual([]);
  });

  it("reports a patch the server refused as not written, and tells no one", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);
    server.patch.mockResolvedValueOnce({ valid: false, error: "no such provider" });

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { a: 2 } },
      choose: keepMine,
    });

    expect(outcome).toMatchObject({
      kind: "saved",
      written: false,
      result: { valid: false, error: "no such provider" },
      raw: baseline,
    });
    expect(heard.changes).toEqual([]);
  });

  it("reports a write it couldn't read back", async () => {
    const server = fakeServer([[providers, baseline]]);
    const coordinator = new ConfigCoordinator(server.io);
    const reads = server.io.read;
    let readsSoFar = 0;
    server.io.read = vi.fn((file: ConfigFile) => {
      // The pre-save read works, the read after the write doesn't.
      if (++readsSoFar > 1) return Promise.reject(new Error("offline"));
      return reads(file);
    });

    const outcome = await coordinator.save(providers, {
      baseline,
      edit: { patch: { a: 2 } },
      choose: keepMine,
    });

    expect(outcome).toMatchObject({ kind: "saved", written: true, raw: null });
  });
});

// ── Subscribers ───────────────────────────────────────────────────────

describe("telling subscribers", () => {
  it("tells them after a write, naming who wrote", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);
    const page = Symbol("page");

    await coordinator.save(providers, {
      baseline: "a = 1\n",
      edit: { patch: { a: 2 } },
      choose: keepMine,
      source: page,
    });
    await coordinator.edit(providers, () => ({ a: 3 }));

    expect(heard.changes).toEqual([
      { file: providers, cause: "write", source: page },
      { file: providers, cause: "write", source: null },
    ]);
  });

  it("tells them after a raw save", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);

    await coordinator.save(providers, {
      baseline: "a = 1\n",
      edit: { text: "a = 2\n" },
      choose: keepMine,
    });

    expect(heard.causes()).toEqual(["write"]);
  });

  it("tells them after a reload, which reads from disk first", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);

    const raw = await coordinator.reload(providers);

    expect(raw).toBe("a = 1\n");
    expect(heard.causes()).toEqual(["reload"]);
    expect(server.log).toEqual(["read providers"]);
  });

  it("tells only the subscribers of the file that changed", async () => {
    const server = fakeServer([
      [providers, "a = 1\n"],
      [config, "b = 1\n"],
      [agentConfigFile("atlas", "providers"), "a = 1\n"],
    ]);
    const coordinator = new ConfigCoordinator(server.io);
    const ofProviders = hear(coordinator, providers);
    const ofConfig = hear(coordinator, config);
    const ofOtherAgent = hear(coordinator, agentConfigFile("atlas", "providers"));

    await coordinator.edit(providers, () => ({ a: 2 }));

    expect(ofProviders.causes()).toEqual(["write"]);
    expect(ofConfig.changes).toEqual([]);
    expect(ofOtherAgent.changes).toEqual([]);
  });

  it("stops telling a subscriber that unsubscribed", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const listener = vi.fn();
    const stop = coordinator.subscribe(providers, listener);

    await coordinator.edit(providers, () => ({ a: 2 }));
    stop();
    await coordinator.edit(providers, () => ({ a: 3 }));

    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("does not let a failing subscriber fail the write or silence the others", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const failure = new Error("subscriber bug");
    coordinator.subscribe(providers, () => {
      throw failure;
    });
    const heard = hear(coordinator, providers);

    const saved = await coordinator.edit(providers, () => ({ a: 2 }));

    expect(saved.written).toBe(true);
    expect(heard.causes()).toEqual(["write"]);
    expect(raised).toEqual([failure]);
  });
});

// ── Restores ──────────────────────────────────────────────────────────

describe("restoring and undoing", () => {
  it("tells subscribers of the config files a restore wrote", async () => {
    const server = fakeServer([
      [providers, "a = 1\n"],
      [config, "b = 1\n"],
    ]);
    const coordinator = new ConfigCoordinator(server.io);
    const ofProviders = hear(coordinator, providers);
    const ofConfig = hear(coordinator, config);
    server.restore.mockImplementationOnce(() => {
      server.outside(providers, "a = 0\n");
      return Promise.resolve({ checkpoint_id: "new", restored_paths: ["providers.toml"] });
    });

    const outcome = await coordinator.restore("scout", "cp1", "agent_config", "providers.toml");

    expect(outcome.restored_paths).toEqual(["providers.toml"]);
    expect(server.restore).toHaveBeenCalledWith("scout", "cp1", "agent_config", "providers.toml");
    expect(ofProviders.changes).toEqual([{ file: providers, cause: "restore", source: null }]);
    expect(ofConfig.changes).toEqual([]);
  });

  it("reads the restored file before telling anyone, so they find it current", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    server.restore.mockImplementationOnce(() =>
      Promise.resolve({ checkpoint_id: "new", restored_paths: ["providers.toml"] }),
    );
    coordinator.subscribe(providers, () => server.log.push("announced"));

    await coordinator.restore("scout", "cp1", "agent_config", "providers.toml");

    expect(server.log).toEqual(["read providers", "announced"]);
  });

  it("tells them about mcp.json when a restore of the workspace's config folder wrote it", async () => {
    const server = fakeServer([[mcp, "{}"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, mcp);
    server.restore.mockResolvedValueOnce({
      checkpoint_id: "new",
      restored_paths: ["config/mcp.json"],
    });

    await coordinator.restore("scout", "cp1", "workspace", "config");

    expect(heard.causes()).toEqual(["restore"]);
  });

  it("tells them about the hub's config when a hub restore wrote it", async () => {
    const server = fakeServer([[HUB_CONFIG_FILE, 'timezone = "UTC"\n']]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, HUB_CONFIG_FILE);
    server.restore.mockResolvedValueOnce({
      checkpoint_id: "new",
      restored_paths: ["config.toml", "secrets.toml.enc"],
    });

    await coordinator.restore(null, "cp1", "hub", "");

    expect(heard.causes()).toEqual(["restore"]);
  });

  it("tells no one about a restore that wrote no config file", async () => {
    const server = fakeServer([[mcp, "{}"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, mcp);
    server.restore.mockResolvedValueOnce({
      checkpoint_id: "new",
      restored_paths: ["notes/a.md"],
    });

    await coordinator.restore("scout", "cp1", "workspace", "notes/a.md");

    expect(heard.changes).toEqual([]);
    expect(server.log).toEqual([]);
  });

  it("tells subscribers of the config files an undo reverted", async () => {
    const server = fakeServer([
      [providers, "a = 1\n"],
      [config, "b = 1\n"],
    ]);
    const coordinator = new ConfigCoordinator(server.io);
    const ofProviders = hear(coordinator, providers);
    const ofConfig = hear(coordinator, config);
    server.undo.mockResolvedValueOnce({
      checkpoint_id: "new",
      reverted_paths: ["config.toml"],
      skipped_paths: ["providers.toml"],
    });

    await coordinator.undo("scout", "cp1", "agent_config");

    expect(server.undo).toHaveBeenCalledWith("scout", "cp1", "agent_config");
    expect(ofConfig.causes()).toEqual(["restore"]);
    // A skipped path was not written.
    expect(ofProviders.changes).toEqual([]);
  });

  it("tells no one when the restore fails", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);
    server.restore.mockRejectedValueOnce(new Error("no such checkpoint"));

    await expect(
      coordinator.restore("scout", "cp1", "agent_config", "providers.toml"),
    ).rejects.toThrow("no such checkpoint");

    expect(heard.changes).toEqual([]);
  });

  it("waits for a write to a file it may restore", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const gate = deferred();
    server.patch.mockImplementationOnce(async () => {
      await gate.promise;
      return { valid: true };
    });

    const writing = coordinator.edit(providers, () => ({ a: 2 }));
    await flush();
    const restoring = coordinator.restore("scout", "cp1", "agent_config", "providers.toml");
    await flush();
    expect(server.restore).not.toHaveBeenCalled();

    gate.release();
    await Promise.all([writing, restoring]);
    expect(server.restore).toHaveBeenCalledTimes(1);
  });

  it("does not hold up a restore of files that aren't config", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const gate = deferred();
    server.patch.mockImplementationOnce(async () => {
      await gate.promise;
      return { valid: true };
    });

    const writing = coordinator.edit(providers, () => ({ a: 2 }));
    await flush();
    await coordinator.restore("scout", "cp1", "workspace", "notes/a.md");

    expect(server.restore).toHaveBeenCalledTimes(1);
    gate.release();
    await writing;
  });
});

// ── Changes made outside ──────────────────────────────────────────────

describe("external changes", () => {
  it("tells subscribers when a file reads differently from when it was last seen", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    await coordinator.reload(providers);
    const heard = hear(coordinator, providers);

    server.outside(providers, "a = 2\n");
    await coordinator.externalChange(providers);

    expect(heard.changes).toEqual([{ file: providers, cause: "external", source: null }]);
  });

  it("stays quiet about the echo of its own write", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    await coordinator.edit(providers, () => ({ a: 2 }));
    const heard = hear(coordinator, providers);

    await coordinator.externalChange(providers);

    expect(heard.changes).toEqual([]);
  });

  it("tells subscribers about a change a save came across but never announced", async () => {
    const server = fakeServer([[providers, "a = 1\nb = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    await coordinator.reload(providers);
    server.outside(providers, "a = 1\nb = 2\n");
    // The save reads the new text and asks the user, who never answers.
    void coordinator.save(providers, {
      baseline: "a = 1\nb = 1\n",
      edit: { patch: { b: 3 } },
      choose: () => new Promise(() => {}),
    });
    await flush();
    const heard = hear(coordinator, providers);

    await coordinator.externalChange(providers);

    expect(heard.causes()).toEqual(["external"]);
  });

  it("tells subscribers about a file it has never read", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);

    await coordinator.externalChange(providers);

    expect(heard.causes()).toEqual(["external"]);
  });

  it("tells subscribers even when the file can't be read, so they report it", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    await coordinator.reload(providers);
    const heard = hear(coordinator, providers);
    server.io.read = () => Promise.reject(new Error("gone"));

    await coordinator.externalChange(providers);

    expect(heard.causes()).toEqual(["external"]);
  });

  it("waits for a write in flight, so the echo of a save isn't taken for a change", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const gate = deferred();
    server.patch.mockImplementationOnce(async (file, diff) => {
      await gate.promise;
      server.disk.set(configFileKey(file), patchText(file, "a = 1\n", diff));
      return { valid: true };
    });
    const heard = hear(coordinator, providers);

    const writing = coordinator.edit(providers, () => ({ a: 2 }));
    await flush();
    const echo = coordinator.externalChange(providers);
    gate.release();
    await Promise.all([writing, echo]);

    expect(heard.causes()).toEqual(["write"]);
  });

  it("tells subscribers of every file it is given on a resync, read again", async () => {
    const server = fakeServer([
      [providers, "a = 1\n"],
      [config, "b = 1\n"],
    ]);
    const coordinator = new ConfigCoordinator(server.io);
    const ofProviders = hear(coordinator, providers);
    const ofConfig = hear(coordinator, config);
    const ofMcp = hear(coordinator, mcp);

    await coordinator.externalResync([providers, config]);

    expect(ofProviders.causes()).toEqual(["external"]);
    expect(ofConfig.causes()).toEqual(["external"]);
    expect(ofMcp.changes).toEqual([]);
    expect(server.log.sort()).toEqual(["read config", "read providers"]);
  });
});

describe("reading a file", () => {
  it("waits for the writes queued before it, and tells no one", async () => {
    const server = fakeServer([[providers, "a = 1\n"]]);
    const coordinator = new ConfigCoordinator(server.io);
    const heard = hear(coordinator, providers);
    const gate = deferred();
    server.patch.mockImplementationOnce(async (file, diff) => {
      await gate.promise;
      server.disk.set(configFileKey(file), patchText(file, "a = 1\n", diff));
      return { valid: true };
    });

    const writing = coordinator.edit(providers, () => ({ a: 2 }));
    const reading = coordinator.read(providers);
    await flush();
    gate.release();
    await writing;

    await expect(reading).resolves.toBe("a = 2\n");
    // Only the write is announced.
    expect(heard.causes()).toEqual(["write"]);
  });
});

describe("where a file's checkpoints are", () => {
  it("names the repository that holds each config file, and its path there", () => {
    expect(checkpointLocationOf(HUB_CONFIG_FILE)).toEqual({ repo: "hub", path: "config.toml" });
    expect(checkpointLocationOf(config)).toEqual({ repo: "agent_config", path: "config.toml" });
    expect(checkpointLocationOf(providers)).toEqual({
      repo: "agent_config",
      path: "providers.toml",
    });
    expect(checkpointLocationOf(mcp)).toEqual({ repo: "workspace", path: "config/mcp.json" });
  });
});

describe("file identity", () => {
  it("tells one agent's files, and the hub's, apart", () => {
    const keys = [
      providers,
      config,
      mcp,
      agentConfigFile("atlas", "providers"),
      HUB_CONFIG_FILE,
    ].map(configFileKey);
    expect(new Set(keys).size).toBe(keys.length);
  });
});
