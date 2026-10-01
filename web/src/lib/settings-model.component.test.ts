import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyPatch } from "../test/apply-patch";
import {
  agentConfigFile,
  ConfigCoordinator,
  configFileKey,
  HUB_CONFIG_FILE,
  type ConfigChooser,
  type ConfigFile,
  type ConfigIo,
} from "./config-coordinator";
import { SettingsModel, type SettingsDeps } from "./settings-model.svelte";
import type { Diagnostic, RepoKind, UndoOutcome, ValidateResponse } from "./types";

// ── A fake server ─────────────────────────────────────────────────────

const AGENT_CONFIG = `timeout_secs = 120

[discord]
token = "secret:discord"

[webhooks.alerts]
secret = "secret:webhook_alerts"
routing = "inbox"
`;

const AGENT_PROVIDERS = `[providers.anthropic]
type = "anthropic"
api_key = "secret:anthropic"

[models]
main = ["anthropic/claude-a", "anthropic/claude-b"]
default = "anthropic/claude-c"
subconscious = { model = ["anthropic/x", "anthropic/y"], temperature = 0.2 }
`;

const AGENT_MCP = '{"mcpServers":{"fs":{"type":"stdio","command":"npx","args":["fs-server"]}}}';

const HUB_CONFIG = `timezone = "UTC"

[cloud]
enabled = true
`;

const scoutConfig = agentConfigFile("scout", "config");
const scoutProviders = agentConfigFile("scout", "providers");
const scoutMcp = agentConfigFile("scout", "mcp");
const brittleConfig = agentConfigFile("brittle", "config");

function format(file: ConfigFile): "toml" | "json" {
  return file.kind === "agent" && file.name === "mcp" ? "json" : "toml";
}

const label = (file: ConfigFile): string =>
  file.kind === "hub" ? "hub" : `${file.agent}/${file.name}`;

interface Fake {
  io: ConfigIo;
  coordinator: ConfigCoordinator;
  model: SettingsModel;
  disk: Map<string, string>;
  /** Writes and undos, in order: `patch scout/providers`, `undo agent_config cp-1`. */
  log: string[];
  /** Refuse the next patch to a file with this verdict. */
  refuse: (file: ConfigFile, verdict: ValidateResponse) => void;
  /** What an undo of each checkpoint reports. */
  undoes: Map<string, UndoOutcome | Error>;
  storeSecret: ReturnType<typeof vi.fn<SettingsDeps["storeSecret"]>>;
  /** Change a file the way something outside the page would, and tell the coordinator. */
  outside: (file: ConfigFile, text: string) => Promise<void>;
}

function fake(extra: [ConfigFile, string][] = []): Fake {
  const disk = new Map<string, string>(
    [
      [scoutConfig, AGENT_CONFIG],
      [scoutProviders, AGENT_PROVIDERS],
      [scoutMcp, AGENT_MCP],
      [HUB_CONFIG_FILE, HUB_CONFIG],
      ...extra,
    ].map(([file, text]) => [configFileKey(file as ConfigFile), text as string]),
  );
  const log: string[] = [];
  const refusals = new Map<string, ValidateResponse>();
  const undoes = new Map<string, UndoOutcome | Error>();
  let checkpoints = 0;
  const io: ConfigIo = {
    read: async (file) => {
      await Promise.resolve();
      return disk.get(configFileKey(file)) ?? "";
    },
    patch: async (file, diff) => {
      log.push(`patch ${label(file)}`);
      await Promise.resolve();
      const refusal = refusals.get(configFileKey(file));
      if (refusal) {
        refusals.delete(configFileKey(file));
        return refusal;
      }
      const key = configFileKey(file);
      disk.set(key, applyPatch(disk.get(key) ?? "", diff, format(file)));
      checkpoints += 1;
      return { valid: true, checkpoint_id: `cp-${checkpoints}` };
    },
    put: async (file, text) => {
      log.push(`put ${label(file)}`);
      await Promise.resolve();
      disk.set(configFileKey(file), text);
      return { valid: true };
    },
    restore: () => Promise.resolve({ checkpoint_id: "restored", restored_paths: [] }),
    undo: async (_agent, id, repo: RepoKind) => {
      log.push(`undo ${repo} ${id}`);
      await Promise.resolve();
      const outcome = undoes.get(id);
      if (outcome instanceof Error) throw outcome;
      return outcome ?? { checkpoint_id: "undone", reverted_paths: [], skipped_paths: [] };
    },
  };
  const coordinator = new ConfigCoordinator(io);
  const storeSecret = vi.fn<SettingsDeps["storeSecret"]>((name) =>
    Promise.resolve({ reference: `secret:${name}` }),
  );
  return {
    io,
    coordinator,
    model: new SettingsModel({ coordinator, storeSecret }),
    disk,
    log,
    undoes,
    storeSecret,
    refuse: (file, verdict) => {
      refusals.set(configFileKey(file), verdict);
    },
    outside: async (file, text) => {
      disk.set(configFileKey(file), text);
      await coordinator.externalChange(file);
      await flush();
    },
  };
}

function first<T>(list: T[]): T {
  const item = list[0];
  if (item === undefined) throw new Error("the list is empty");
  return item;
}

/** Let every queued promise callback run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 30; i++) await Promise.resolve();
}

const keepMine: ConfigChooser = () => Promise.resolve("keep-mine");
const useDisk: ConfigChooser = () => Promise.resolve("use-disk");

async function loadedScout(f: Fake): Promise<ReturnType<SettingsModel["agent"]>> {
  const scope = f.model.agent("scout");
  await scope.load();
  return scope;
}

beforeEach(() => {
  // An error that reaches the user is also logged; keep the test output clean.
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ── Loading and the forms ─────────────────────────────────────────────

describe("loading a scope", () => {
  it("parses each file into a form and keeps its text as the baseline", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    expect(scope.config.timeout_secs).toBe("120");
    expect(scope.providers.map((p) => p.name)).toEqual(["anthropic"]);
    expect(scope.models.default).toBe("anthropic/claude-c");
    expect(scope.mcpServers.map((s) => s.name)).toEqual(["fs"]);
    expect(scope.configFile.raw).toBe(AGENT_CONFIG);
    expect(scope.dirty).toBe(false);
  });

  it("keeps the install's values apart, read-only, and never in the agent's form", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    expect(scope.install.timezone).toBe("UTC");
    expect(scope.config.timezone).toBe("");
  });

  it("reports a file it couldn't read, and still loads the rest", async () => {
    const f = fake();
    const read = f.io.read;
    f.io.read = (file) =>
      configFileKey(file) === configFileKey(scoutMcp)
        ? Promise.reject(new TypeError("down"))
        : read(file);
    const scope = await loadedScout(f);

    expect(scope.loadError).toContain("Residuum isn't reachable");
    expect(scope.config.timeout_secs).toBe("120");
  });

  it("reports the hub's file when the install's values can't be read", async () => {
    const f = fake();
    const read = f.io.read;
    f.io.read = (file) =>
      file.kind === "hub" ? Promise.reject(new TypeError("down")) : read(file);
    const scope = await loadedScout(f);

    expect(scope.loadError).toContain("Couldn't read the install-wide settings.");
    expect(scope.configFile.loadError).toBeNull();
  });

  it("flags a file whose text doesn't parse", async () => {
    const f = fake([[scoutConfig, "timeout_secs = = 1"]]);
    const scope = await loadedScout(f);

    expect(scope.configFile.unreadable).toBe(true);
    expect(scope.providersFile.unreadable).toBe(false);
  });
});

// ── Staging ───────────────────────────────────────────────────────────

describe("a diff per file", () => {
  it("gives each file its own patch and leaves the others empty", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    scope.config.timeout_secs = "90";
    expect(scope.configFile.patch).toEqual({ timeout_secs: 90 });
    expect(scope.providersFile.patch).toEqual({});
    expect(scope.mcpFile.patch).toEqual({});

    first(scope.providers).url = "http://localhost:1";
    scope.mcpServers.push({
      name: "git",
      transport: "stdio",
      command: "git-mcp",
      args: [],
      env: {},
    });

    expect(scope.providersFile.patch).toEqual({
      providers: { anthropic: { url: "http://localhost:1" } },
    });
    expect(scope.mcpFile.patch).toEqual({
      mcpServers: { git: { type: "stdio", command: "git-mcp" } },
    });
    expect(scope.dirty).toBe(true);
  });

  it("stages the removal of a provider, a server and a webhook", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    scope.providers.splice(0, 1);
    scope.mcpServers.splice(0, 1);
    scope.config.webhooks.splice(0, 1);

    expect(scope.providersFile.patch).toMatchObject({ providers: { anthropic: null } });
    expect(scope.mcpFile.patch).toEqual({ mcpServers: { fs: null } });
    expect(scope.configFile.patch).toEqual({ webhooks: { alerts: null } });
  });
});

describe("skill and tool folders", () => {
  it("stages a folder's removal, and Discard brings it back", async () => {
    const f = fake([
      [
        scoutConfig,
        `${AGENT_CONFIG.split("\n[webhooks")[0]}\n[skills]\ndirs = ["/a", "/b"]\n\n[tools]\npath = ["/bin"]\n`,
      ],
    ]);
    const scope = await loadedScout(f);
    expect(scope.config.skills_dirs).toEqual(["/a", "/b"]);

    scope.config.skills_dirs.splice(0, 1);
    scope.config.tools_path.splice(0, 1);

    expect(scope.configFile.patch).toEqual({ skills: { dirs: ["/b"] }, tools: { path: null } });
    scope.discard();
    expect(scope.config.skills_dirs).toEqual(["/a", "/b"]);
    expect(scope.config.tools_path).toEqual(["/bin"]);
    expect(scope.dirty).toBe(false);
  });
});

describe("staged changes across scopes", () => {
  it("keeps what is staged in a scope while another is open, and when it comes back", async () => {
    const f = fake([[brittleConfig, "timeout_secs = 30\n"]]);
    const scout = await loadedScout(f);
    scout.config.timeout_secs = "90";

    const hub = f.model.scope("_all");
    await hub.load();
    const brittle = f.model.scope("brittle");
    await brittle.load();
    brittle.config.timeout_secs = "45";

    expect(hub.dirty).toBe(false);
    expect(f.model.scope("scout")).toBe(scout);
    expect(scout.config.timeout_secs).toBe("90");
    expect(f.model.stagedScopes).toEqual([scout, brittle]);
  });

  it("keeps staged changes when the scope is loaded again, and its baseline with them", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";

    await f.outside(scoutConfig, AGENT_CONFIG.replace("120", "150"));
    await scope.load();

    expect(scope.config.timeout_secs).toBe("90");
    expect(scope.configFile.raw).toBe(AGENT_CONFIG);
    expect(scope.configFile.changedOnDisk).toBe(true);
  });
});

describe("Discard", () => {
  it("brings back changes and removals in every file, and clears the problems", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    scope.providers.splice(0, 1);
    scope.mcpServers.splice(0, 1);
    scope.models.main = "anthropic/other";

    scope.discard();

    expect(scope.dirty).toBe(false);
    expect(scope.config.timeout_secs).toBe("120");
    expect(scope.providers.map((p) => p.name)).toEqual(["anthropic"]);
    expect(scope.mcpServers.map((s) => s.name)).toEqual(["fs"]);
    expect(scope.models.main).toBe("anthropic/claude-a");
  });

  it("only touches its own scope", async () => {
    const f = fake([[brittleConfig, "timeout_secs = 30\n"]]);
    const scout = await loadedScout(f);
    const brittle = f.model.agent("brittle");
    await brittle.load();
    scout.config.timeout_secs = "90";
    brittle.config.timeout_secs = "45";

    scout.discard();

    expect(brittle.config.timeout_secs).toBe("45");
  });

  it("is what Reload does before it reads the files again", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.disk.set(configFileKey(scoutConfig), AGENT_CONFIG.replace("120", "150"));

    await scope.reload();

    expect(scope.config.timeout_secs).toBe("150");
    expect(scope.dirty).toBe(false);
  });
});

// ── Saving ────────────────────────────────────────────────────────────

describe("Save", () => {
  it("writes providers, then config, then MCP servers, and reads each back as the baseline", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.mcpServers).command = "other";
    scope.config.timeout_secs = "90";
    first(scope.providers).url = "http://localhost:1";

    const result = await scope.save(keepMine);

    expect(f.log).toEqual(["patch scout/providers", "patch scout/config", "patch scout/mcp"]);
    expect(result.outcome).toBe("saved");
    expect(result.files.map((file) => file.status)).toEqual(["saved", "saved", "saved"]);
    expect(result.message).toBe("Saved providers.toml, config.toml and mcp.json.");
    expect(scope.dirty).toBe(false);
    expect(scope.configFile.raw).toBe(f.disk.get(configFileKey(scoutConfig)));
    expect(scope.saving).toBe(false);
  });

  it("only writes the files with staged changes", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";

    await scope.save(keepMine);

    expect(f.log).toEqual(["patch scout/config"]);
  });

  it("says there is nothing to save when nothing is staged", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    const result = await scope.save(keepMine);

    expect(result.outcome).toBe("nothing");
    expect(f.log).toEqual([]);
  });

  it("returns every checkpoint the save produced, in the order it took them", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    first(scope.providers).url = "http://localhost:1";
    first(scope.mcpServers).command = "other";

    const result = await scope.save(keepMine);

    expect(result.checkpoints).toEqual([
      { file: "providers", repo: "agent_config", id: "cp-1" },
      { file: "config", repo: "agent_config", id: "cp-2" },
      { file: "mcp", repo: "workspace", id: "cp-3" },
    ]);
    expect(scope.undoable).toBe(true);
  });

  it("gives a second call the result of the save already running", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";

    const [one, two] = await Promise.all([scope.save(keepMine), scope.save(keepMine)]);

    expect(two).toBe(one);
    expect(f.log).toEqual(["patch scout/config"]);
  });

  it("keeps what the user edited while the save ran, and takes the rest from the file", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    const saving = scope.save(keepMine);
    await flush();
    // The page's inputs stay live during a save.
    scope.config.max_tokens = "4000";
    await saving;

    expect(scope.config.timeout_secs).toBe("90");
    expect(scope.configFile.patch).toEqual({ max_tokens: 4000 });
  });

  it("lets a change to other keys made meanwhile through, and shows it afterwards", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.disk.set(configFileKey(scoutConfig), `max_tokens = 2000\n${AGENT_CONFIG}`);

    await scope.save(keepMine);

    expect(f.disk.get(configFileKey(scoutConfig))).toContain("max_tokens = 2000");
    expect(scope.config.max_tokens).toBe("2000");
    expect(scope.dirty).toBe(false);
  });

  it("asks which to keep when a key being saved changed on disk, and drops the changes if the disk is chosen", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.disk.set(configFileKey(scoutConfig), AGENT_CONFIG.replace("120", "150"));
    const choose = vi.fn<ConfigChooser>(useDisk);

    const result = await scope.save(choose);

    expect(choose).toHaveBeenCalledWith(expect.objectContaining({ keys: ["timeout_secs"] }));
    expect(result.files[0]?.status).toBe("used-disk");
    expect(scope.config.timeout_secs).toBe("150");
    expect(scope.dirty).toBe(false);
  });
});

describe("a partial failure", () => {
  it("names the files that saved and the ones that didn't, and keeps the failed file staged", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.providers).url = "http://localhost:1";
    scope.config.timeout_secs = "90";
    first(scope.mcpServers).command = "other";
    f.refuse(scoutConfig, { valid: false, error: "timeout_secs must be above zero" });

    const result = await scope.save(keepMine);

    expect(result.outcome).toBe("partial");
    expect(result.files.map((file) => [file.label, file.status])).toEqual([
      ["providers.toml", "saved"],
      ["config.toml", "failed"],
      ["mcp.json", "saved"],
    ]);
    expect(result.message).toBe(
      "Saved providers.toml and mcp.json. Couldn't save config.toml: timeout_secs must be above zero. Changes that weren't saved are still staged.",
    );
    expect(scope.providersFile.dirty).toBe(false);
    expect(scope.mcpFile.dirty).toBe(false);
    expect(scope.configFile.dirty).toBe(true);
    // Only the files that wrote took checkpoints.
    expect(result.checkpoints.map((c) => c.file)).toEqual(["providers", "mcp"]);
    expect(scope.lastResult).toBe(result);
  });

  it("doesn't try config.toml when providers.toml failed, because it is checked against it", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.providers).url = "http://localhost:1";
    scope.config.timeout_secs = "90";
    first(scope.mcpServers).command = "other";
    f.refuse(scoutProviders, { valid: false, error: "bad provider" });

    const result = await scope.save(keepMine);

    expect(f.log).toEqual(["patch scout/providers", "patch scout/mcp"]);
    expect(result.files.map((file) => file.status)).toEqual(["failed", "skipped", "saved"]);
    expect(result.outcome).toBe("partial");
    expect(scope.configFile.dirty).toBe(true);
  });

  it("reports a request that didn't get through in plain language", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.io.patch = () => Promise.reject(new TypeError("Failed to fetch"));

    const result = await scope.save(keepMine);

    expect(result.outcome).toBe("failed");
    expect(result.message).toContain("Couldn't save config.toml. Residuum isn't reachable");
    expect(scope.configFile.dirty).toBe(true);
    expect(scope.saving).toBe(false);
  });
});

// ── Diagnostics ───────────────────────────────────────────────────────

function error(message: string, path?: string): Diagnostic {
  return path === undefined
    ? { severity: "error", message }
    : { severity: "error", message, location: { kind: "path", path } };
}

describe("diagnostics from a save", () => {
  it("puts one on the field its key path names", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.mcpServers).command = "other";
    first(scope.providers).url = "nope";
    f.refuse(scoutMcp, {
      valid: false,
      error: "mcpServers.fs: bad",
      diagnostics: [error("unknown transport", "mcpServers.fs")],
    });
    f.refuse(scoutProviders, {
      valid: false,
      error: "bad model",
      diagnostics: [
        error("no such provider", "models.main"),
        error("bad url", "providers.anthropic.url"),
      ],
    });

    await scope.save(keepMine);

    expect(scope.fieldDiagnostics({ kind: "mcp", name: "fs" }).map((d) => d.message)).toEqual([
      "unknown transport",
    ]);
    expect(scope.fieldDiagnostics({ kind: "role", role: "main" }).map((d) => d.message)).toEqual([
      "no such provider",
    ]);
    expect(
      scope
        .fieldDiagnostics({ kind: "provider", name: "anthropic", field: "url" })
        .map((d) => d.message),
    ).toEqual(["bad url"]);
    expect(scope.fieldDiagnostics({ kind: "role", role: "default" })).toEqual([]);
  });

  it("reports the rest at the top of a section: those with no key path anywhere, those whose key no field holds in that section", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.refuse(scoutConfig, {
      valid: false,
      error: "line 3: expected a value",
      diagnostics: [
        { severity: "error", message: "expected a value", location: { kind: "line", line: 3 } },
        error("unknown key", "retry.bogus"),
        error("the model is missing"),
      ],
    });

    await scope.save(keepMine);

    expect(scope.sectionDiagnostics("runtime").map((d) => d.message)).toEqual([
      "expected a value",
      "unknown key",
      "the model is missing",
    ]);
    // The key belongs to Runtime; the rest are shown wherever the user is.
    expect(scope.sectionDiagnostics("memory").map((d) => d.message)).toEqual([
      "expected a value",
      "the model is missing",
    ]);
    expect(scope.diagnostics).toHaveLength(3);
  });

  it("raises the server's error as a diagnostic when it sent none", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.refuse(scoutConfig, { valid: false, error: "couldn't apply the change to config.toml" });

    await scope.save(keepMine);

    expect(scope.sectionDiagnostics("runtime").map((d) => d.message)).toEqual([
      "couldn't apply the change to config.toml",
    ]);
  });

  it("clears a file's problems once it saves, and every problem on Discard", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.refuse(scoutConfig, {
      valid: false,
      error: "bad",
      diagnostics: [error("bad", "retry.max_retries")],
    });
    await scope.save(keepMine);
    expect(scope.fieldDiagnostics({ kind: "config", field: "retry_max_retries" })).toHaveLength(1);

    await scope.save(keepMine);
    expect(scope.diagnostics).toEqual([]);

    scope.config.timeout_secs = "10";
    f.refuse(scoutConfig, { valid: false, error: "bad again" });
    await scope.save(keepMine);
    scope.discard();
    expect(scope.diagnostics).toEqual([]);
  });

  it("keeps a warning from a save that succeeded", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.io.patch = () =>
      Promise.resolve({
        valid: true,
        checkpoint_id: "cp",
        diagnostics: [
          {
            severity: "warning",
            message: "deprecated",
            location: { kind: "path", path: "timeout_secs" },
          },
        ],
      });

    await scope.save(keepMine);

    expect(scope.fieldDiagnostics({ kind: "config", field: "timeout_secs" })[0]?.severity).toBe(
      "warning",
    );
  });
});

// ── Scope isolation ───────────────────────────────────────────────────

describe("scope isolation", () => {
  it("never writes the install's file from an agent, whatever the agent's form holds", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    // Install-wide keys, put into the agent's form.
    scope.config.timezone = "America/Chicago";
    scope.config.cloud_relay_url = "wss://elsewhere";
    scope.config.tracing_log_level = "trace";
    scope.config.gateway_port = "9000";
    scope.config.bg_max_concurrent = "9";
    scope.config.a2a_port = "9001";

    expect(scope.dirty).toBe(false);
    const result = await scope.save(keepMine);

    expect(result.outcome).toBe("nothing");
    expect(f.log).toEqual([]);
  });

  it("never writes an agent's file from All agents", async () => {
    const f = fake();
    const hub = f.model.all();
    await hub.load();
    hub.config.timezone = "America/Chicago";
    hub.config.timeout_secs = "5";
    hub.config.discord_token = "secret:other";

    expect(hub.configFile.patch).toEqual({ timezone: "America/Chicago" });
    await hub.save(keepMine);

    expect(f.log).toEqual(["patch hub"]);
    expect(f.disk.get(configFileKey(scoutConfig))).toBe(AGENT_CONFIG);
  });

  it("writes one agent's files without touching another's", async () => {
    const f = fake([[brittleConfig, "timeout_secs = 30\n"]]);
    const scout = await loadedScout(f);
    const brittle = f.model.agent("brittle");
    await brittle.load();
    brittle.config.timeout_secs = "45";
    scout.config.timeout_secs = "90";

    await scout.save(keepMine);

    expect(f.log).toEqual(["patch scout/config"]);
    expect(brittle.dirty).toBe(true);
  });
});

// ── Undo ──────────────────────────────────────────────────────────────

describe("Undo", () => {
  async function savedThreeFiles(f: Fake): Promise<ReturnType<SettingsModel["agent"]>> {
    const scope = await loadedScout(f);
    first(scope.providers).url = "http://localhost:1";
    scope.config.timeout_secs = "90";
    first(scope.mcpServers).command = "other";
    await scope.save(keepMine);
    return scope;
  }

  it("restores every checkpoint in reverse save order, with one path skipped", async () => {
    const f = fake();
    const scope = await savedThreeFiles(f);
    f.log.length = 0;
    f.undoes.set("cp-3", {
      checkpoint_id: "u3",
      reverted_paths: ["config/mcp.json"],
      skipped_paths: [],
    });
    f.undoes.set("cp-2", {
      checkpoint_id: "u2",
      reverted_paths: ["config.toml"],
      skipped_paths: [],
    });
    f.undoes.set("cp-1", {
      checkpoint_id: "u1",
      reverted_paths: [],
      skipped_paths: ["providers.toml"],
    });

    const result = await scope.undo();

    expect(f.log).toEqual([
      "undo workspace cp-3",
      "undo agent_config cp-2",
      "undo agent_config cp-1",
    ]);
    expect(result.files).toEqual([
      { file: "mcp", label: "mcp.json", reverted: ["config/mcp.json"], skipped: [], error: null },
      { file: "config", label: "config.toml", reverted: ["config.toml"], skipped: [], error: null },
      {
        file: "providers",
        label: "providers.toml",
        reverted: [],
        skipped: ["providers.toml"],
        error: null,
      },
    ]);
    expect(result.failed).toEqual([]);
    expect(result.message).toBe(
      "Reverted mcp.json and config.toml. Skipped providers.toml in providers.toml: changed again since.",
    );
    expect(scope.undoable).toBe(false);
  });

  it("names a file it couldn't restore, restores the others, and keeps that checkpoint for another try", async () => {
    const f = fake();
    const scope = await savedThreeFiles(f);
    f.undoes.set("cp-2", new TypeError("down"));

    const result = await scope.undo();

    expect(result.files.map((file) => file.file)).toEqual(["mcp", "config", "providers"]);
    expect(result.failed).toEqual(["config"]);
    expect(result.files[1]?.error).toContain("Residuum isn't reachable");
    expect(result.message).toContain("Couldn't restore config.toml.");
    expect(scope.lastResult?.checkpoints).toEqual([
      { file: "config", repo: "agent_config", id: "cp-2" },
    ]);
    expect(scope.undoable).toBe(true);
  });

  it("refreshes the forms of the files it restored", async () => {
    const f = fake();
    const scope = await savedThreeFiles(f);
    f.io.undo = async () => {
      f.disk.set(configFileKey(scoutConfig), AGENT_CONFIG);
      await Promise.resolve();
      return {
        checkpoint_id: "u",
        reverted_paths: ["config.toml"],
        skipped_paths: [],
      } satisfies UndoOutcome;
    };

    await scope.undo();
    await flush();

    expect(scope.config.timeout_secs).toBe("120");
  });

  it("does nothing when the last save took no checkpoint", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    f.io.patch = () => Promise.resolve({ valid: true });
    await scope.save(keepMine);

    const result = await scope.undo();

    expect(result.files).toEqual([]);
    expect(result.message).toBe("Nothing needed undoing.");
  });
});

// ── Failover lists ────────────────────────────────────────────────────

describe("what the form doesn't show", () => {
  it("leaves failover lists and models.default alone when other fields are saved", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.providers).url = "http://localhost:1";
    scope.models.pulse = "anthropic/claude-p";

    await scope.save(keepMine);

    const saved = f.disk.get(configFileKey(scoutProviders)) ?? "";
    expect(saved).toContain('"anthropic/claude-a"');
    expect(saved).toContain('"anthropic/claude-b"');
    expect(saved).toContain('default = "anthropic/claude-c"');
    expect(saved).toContain('"anthropic/y"');
  });

  it("keeps the rest of a list when its first model is changed", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.models.main = "anthropic/claude-new";

    expect(scope.providersFile.patch).toEqual({
      models: { main: ["anthropic/claude-new", "anthropic/claude-b"] },
    });
    await scope.save(keepMine);
    expect(f.disk.get(configFileKey(scoutProviders))).toContain('"anthropic/claude-b"');
  });

  it("keeps a list when an override is added to it, and when one is changed", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.models.overrides.main = { temperature: "0.5", thinking: "" };
    scope.models.overrides.subconscious = { temperature: "0.4", thinking: "low" };

    expect(scope.providersFile.patch).toEqual({
      models: {
        main: {
          model: ["anthropic/claude-a", "anthropic/claude-b"],
          temperature: 0.5,
          thinking: null,
        },
        subconscious: {
          model: ["anthropic/x", "anthropic/y"],
          temperature: 0.4,
          thinking: "low",
        },
      },
    });
    await scope.save(keepMine);
    const saved = f.disk.get(configFileKey(scoutProviders)) ?? "";
    expect(saved).toContain('"anthropic/claude-b"');
    expect(saved).toContain('"anthropic/y"');
    expect(scope.models.fallbacks.main).toEqual(["anthropic/claude-b"]);
    expect(scope.models.overrides).toMatchObject({ main: { temperature: "0.5" } });
  });
});

// ── Typed secrets ─────────────────────────────────────────────────────

describe("typed secrets", () => {
  it("stores a typed token and writes its reference instead", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.telegram_token = "123:abc";
    scope.config.ws_brave_api_key = "brave-key";

    const result = await scope.save(keepMine);

    expect(f.storeSecret.mock.calls).toEqual([
      ["telegram", "123:abc"],
      ["ws_brave", "brave-key"],
    ]);
    expect(result.storedSecrets).toEqual(["telegram", "ws_brave"]);
    const saved = f.disk.get(configFileKey(scoutConfig)) ?? "";
    expect(saved).toContain("secret:telegram");
    expect(saved).not.toContain("123:abc");
    expect(scope.config.telegram_token).toBe("secret:telegram");
  });

  it("stores webhook secrets and provider keys under their own names", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.webhooks.push({
      name: "deploys",
      secret: "hook-secret",
      routing: "inbox",
      format: "parsed",
      content_fields: "",
    });
    scope.providers.push({
      name: "openai",
      type: "openai",
      apiKey: "sk-123",
      url: "",
      keepAlive: "",
    });
    scope.providers.push({
      name: "local",
      type: "ollama",
      apiKey: "not-a-secret",
      url: "",
      keepAlive: "",
    });

    await scope.save(keepMine);

    expect(f.storeSecret.mock.calls).toEqual([
      ["openai", "sk-123"],
      ["webhook_deploys", "hook-secret"],
    ]);
    expect(f.disk.get(configFileKey(scoutProviders))).toContain("secret:openai");
  });

  it("leaves references, unchanged values and empty ones alone", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.discord_token = "${DISCORD_TOKEN}";
    scope.config.telegram_token = "secret:mine";
    scope.config.ws_tavily_api_key = "";

    await scope.save(keepMine);

    expect(f.storeSecret).not.toHaveBeenCalled();
  });

  it("stores the Cloud token from All agents, and never from an agent", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.cloud_token = "tok";
    await scope.save(keepMine);
    expect(f.storeSecret).not.toHaveBeenCalled();

    const hub = f.model.all();
    await hub.load();
    hub.config.cloud_token = "tok";
    await hub.save(keepMine);
    expect(f.storeSecret).toHaveBeenCalledWith("cloud_token", "tok");
  });

  it("counts a key typed over an existing one as saved, though the file's reference doesn't change", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.discord_token = "new-token";

    const result = await scope.save(keepMine);

    expect(f.storeSecret).toHaveBeenCalledWith("discord", "new-token");
    expect(result.outcome).toBe("saved");
    expect(result.files[0]?.status).toBe("unchanged");
    expect(result.message).toBe("Saved the key.");
    expect(f.log).toEqual([]);
  });

  it("writes nothing when a secret can't be stored", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.telegram_token = "123:abc";
    scope.config.timeout_secs = "90";
    f.storeSecret.mockRejectedValueOnce(new TypeError("down"));

    const result = await scope.save(keepMine);

    expect(result.outcome).toBe("failed");
    expect(result.message).toContain("Nothing was saved");
    expect(f.log).toEqual([]);
    expect(scope.configFile.dirty).toBe(true);
  });
});

// ── Raw versus form ───────────────────────────────────────────────────

describe("raw and form locking", () => {
  it("makes a file's raw editor read-only while the form has changes to it", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    expect(scope.configFile.lockedBy).toBeNull();

    scope.config.timeout_secs = "90";

    expect(scope.configFile.lockedBy).toBe("form");
    // Other files of the scope stay open.
    expect(scope.providersFile.lockedBy).toBeNull();
    scope.discard();
    expect(scope.configFile.lockedBy).toBeNull();
  });

  it("makes the form read-only while the raw editor holds a draft, until it is the file again", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    scope.configFile.setRawDraft("timeout_secs = 1\n");
    expect(scope.configFile.lockedBy).toBe("raw");

    scope.configFile.setRawDraft(AGENT_CONFIG);
    expect(scope.configFile.lockedBy).toBeNull();
    expect(scope.configFile.rawDraft).toBeNull();
  });

  it("drops a draft that becomes the file, as when a raw save writes it", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.configFile.setRawDraft("timeout_secs = 1\n");

    await f.outside(scoutConfig, "timeout_secs = 1\n");

    expect(scope.configFile.rawDraft).toBeNull();
    expect(scope.config.timeout_secs).toBe("1");
  });
});

// ── Changes made elsewhere ────────────────────────────────────────────

describe("external changes", () => {
  it("refreshes the baseline and form of a file with nothing staged", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    await f.outside(scoutConfig, AGENT_CONFIG.replace("120", "150"));

    expect(scope.config.timeout_secs).toBe("150");
    expect(scope.configFile.raw).toContain("150");
    expect(scope.dirty).toBe(false);
  });

  it("leaves a file with staged changes alone, and says it changed", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";

    await f.outside(scoutConfig, AGENT_CONFIG.replace("120", "150"));

    expect(scope.config.timeout_secs).toBe("90");
    expect(scope.configFile.raw).toBe(AGENT_CONFIG);
    expect(scope.configFile.changedOnDisk).toBe(true);
    expect(scope.configFile.patch).toEqual({ timeout_secs: 90 });
  });

  it("lets the coordinator's re-read catch the clash when that file is saved", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    await f.outside(scoutConfig, AGENT_CONFIG.replace("120", "150"));
    const choose = vi.fn<ConfigChooser>(keepMine);

    await scope.save(choose);

    expect(choose).toHaveBeenCalledWith(expect.objectContaining({ keys: ["timeout_secs"] }));
    expect(scope.configFile.changedOnDisk).toBe(false);
  });

  it("refreshes only the file that changed", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    first(scope.providers).url = "http://localhost:1";

    await f.outside(scoutMcp, '{"mcpServers":{}}');

    expect(scope.mcpServers).toEqual([]);
    expect(scope.providers[0]?.url).toBe("http://localhost:1");
  });

  it("doesn't read a file again for a write it made itself", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";
    const read = vi.spyOn(f.io, "read");

    await scope.save(keepMine);
    await flush();

    // The coordinator reads before the write, and again after it. Nothing
    // else does: the model took the text from the save.
    expect(read.mock.calls.map(([file]) => label(file))).toEqual(["scout/config", "scout/config"]);
  });

  it("refreshes for another view's write, and waits while changes are staged", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.max_tokens = "4000";

    // Something else, such as the composer, writes the file.
    await f.coordinator.edit(scoutConfig, () => ({ timeout_secs: 200 }));
    await flush();

    expect(scope.config.max_tokens).toBe("4000");
    expect(scope.config.timeout_secs).toBe("120");
    expect(scope.configFile.changedOnDisk).toBe(true);

    scope.discard();
    await f.coordinator.edit(scoutConfig, () => ({ timeout_secs: 300 }));
    await flush();

    expect(scope.config.timeout_secs).toBe("300");
    expect(scope.configFile.changedOnDisk).toBe(false);
  });

  it("follows the hub's file for the install's values, whatever is staged", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    scope.config.timeout_secs = "90";

    await f.outside(HUB_CONFIG_FILE, 'timezone = "Asia/Tokyo"\n');

    expect(scope.install.timezone).toBe("Asia/Tokyo");
    expect(scope.config.timeout_secs).toBe("90");
  });

  it("shows another scope's save to the hub in an agent's install values", async () => {
    const f = fake();
    const scope = await loadedScout(f);
    const hub = f.model.all();
    await hub.load();
    hub.config.timezone = "Asia/Tokyo";

    await hub.save(keepMine);
    await flush();

    expect(scope.install.timezone).toBe("Asia/Tokyo");
  });

  it("stops following once the scope is dropped", async () => {
    const f = fake();
    const scope = await loadedScout(f);

    f.model.drop("scout");
    await f.outside(scoutConfig, AGENT_CONFIG.replace("120", "150"));

    expect(scope.config.timeout_secs).toBe("120");
    expect(f.model.agent("scout")).not.toBe(scope);
  });
});
