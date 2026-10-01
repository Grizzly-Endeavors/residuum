import { describe, expect, it } from "vitest";
import {
  defaultConfigFields,
  defaultModels,
  diffConfigFields,
  diffMcpServers,
  diffProviders,
  modelRoleJson,
  configFieldOwner,
  parseConfigToml,
  parseProvidersToml,
  splitConfigPatch,
  parseMcpJson,
} from "./settings-toml";
import type { McpServerEntry, SettingsProviderEntry } from "./types";

function provider(overrides: Partial<SettingsProviderEntry> = {}): SettingsProviderEntry {
  return { name: "", type: "anthropic", apiKey: "", url: "", keepAlive: "", ...overrides };
}

function mcpServer(overrides: Partial<McpServerEntry> = {}): McpServerEntry {
  return { name: "", transport: "stdio", command: "npx", args: [], env: {}, ...overrides };
}

describe("diffConfigFields", () => {
  it("is empty when nothing changed", () => {
    const fields = defaultConfigFields();
    expect(diffConfigFields(fields, fields)).toEqual({});
  });

  it("emits only the field that changed, nested at its TOML path", () => {
    const baseline = defaultConfigFields();
    const current = { ...baseline, gateway_port: "8080" };
    expect(diffConfigFields(baseline, current)).toEqual({ gateway: { port: 8080 } });
  });

  it("reads a number the form holds as a number, as a bound number input yields", () => {
    const baseline = defaultConfigFields();
    const current = { ...baseline, gateway_port: 8080 as unknown as string };
    expect(diffConfigFields(baseline, current)).toEqual({ gateway: { port: 8080 } });
  });

  it("carries the tracing fields to the hub's [tracing] table", () => {
    const baseline = defaultConfigFields();
    const current = {
      ...baseline,
      tracing_log_level: "trace",
      tracing_auto_error_reporting: true,
      tracing_sanitize_content: false,
    };
    expect(diffConfigFields(baseline, current)).toEqual({
      tracing: { log_level: "trace", auto_error_reporting: true, sanitize_content: false },
    });
  });

  it("emits null to clear a field back to empty", () => {
    const baseline = { ...defaultConfigFields(), discord_token: "abc" };
    const current = { ...baseline, discord_token: "" };
    expect(diffConfigFields(baseline, current)).toEqual({ discord: { token: null } });
  });

  it("collapses a value that equals its non-empty default to null", () => {
    const baseline = defaultConfigFields();
    const current = { ...baseline, teams_port: "7701" };
    // 7701 is the default, so setting it explicitly is equivalent to unset
    expect(diffConfigFields(baseline, current)).toEqual({});
  });

  it("does not touch unrelated fields in the same section", () => {
    const baseline = { ...defaultConfigFields(), gateway_bind: "127.0.0.1", gateway_port: "7700" };
    const current = { ...baseline, gateway_port: "8080" };
    const diff = diffConfigFields(baseline, current);
    expect(diff).toEqual({ gateway: { port: 8080 } });
    expect(diff.gateway).not.toHaveProperty("bind");
  });

  it("diffs a float field as a float literal", () => {
    const baseline = defaultConfigFields();
    const current = { ...baseline, search_vector_weight: "0.7" };
    expect(diffConfigFields(baseline, current)).toEqual({
      memory: { search: { vector_weight: 0.7 } },
    });
  });

  it("round-trips the artifact idle timeout through parse + diff", () => {
    const fields = parseConfigToml("[background]\nidle_timeout_artifact_minutes = 25\n");
    expect(fields.bg_idle_timeout_artifact_minutes).toBe("25");
    const changed = { ...fields, bg_idle_timeout_artifact_minutes: "30" };
    expect(diffConfigFields(fields, changed)).toEqual({
      background: { idle_timeout_artifact_minutes: 30 },
    });
  });

  it("diffs a new webhook as a full entry", () => {
    const baseline = defaultConfigFields();
    const current = {
      ...baseline,
      webhooks: [
        { name: "gh", secret: "s3cr3t", routing: "inbox", format: "parsed", content_fields: "" },
      ],
    };
    expect(diffConfigFields(baseline, current)).toEqual({
      webhooks: { gh: { secret: "s3cr3t" } },
    });
  });

  it("removes a deleted webhook by name, leaving the map otherwise empty", () => {
    const baseline = {
      ...defaultConfigFields(),
      webhooks: [
        { name: "gh", secret: "s3cr3t", routing: "inbox", format: "parsed", content_fields: "" },
      ],
    };
    const current = { ...baseline, webhooks: [] };
    expect(diffConfigFields(baseline, current)).toEqual({ webhooks: { gh: null } });
  });

  it("diffs one changed field on an existing webhook without replacing the whole entry", () => {
    const baseline = {
      ...defaultConfigFields(),
      webhooks: [
        { name: "gh", secret: "s3cr3t", routing: "inbox", format: "parsed", content_fields: "" },
      ],
    };
    const current = {
      ...baseline,
      webhooks: [
        { name: "gh", secret: "s3cr3t", routing: "queue", format: "parsed", content_fields: "" },
      ],
    };
    expect(diffConfigFields(baseline, current)).toEqual({ webhooks: { gh: { routing: "queue" } } });
  });
});

describe("modelRoleJson", () => {
  it("is a plain string with no overrides", () => {
    expect(modelRoleJson("anthropic/claude-opus")).toBe("anthropic/claude-opus");
  });

  it("clears to null when empty", () => {
    expect(modelRoleJson("")).toBeNull();
  });

  it("becomes an $inline table when an override is set", () => {
    expect(modelRoleJson("anthropic/claude-opus", { temperature: "0.7", thinking: "" })).toEqual({
      $inline: { model: "anthropic/claude-opus", temperature: 0.7 },
    });
  });

  it("includes both overrides when both are set", () => {
    expect(
      modelRoleJson("anthropic/claude-opus", { temperature: "0.7", thinking: "high" }),
    ).toEqual({
      $inline: { model: "anthropic/claude-opus", temperature: 0.7, thinking: "high" },
    });
  });
});

describe("failover lists", () => {
  const raw = `[models]
main = ["anthropic/a", "openai/b", "gemini/c"]
default = "anthropic/d"
observer = { model = ["anthropic/o1", "anthropic/o2"], temperature = 0.2 }

[background.models]
small = ["anthropic/s1", "anthropic/s2"]
`;

  it("shows the first model of a list and keeps the rest", () => {
    const { models } = parseProvidersToml(raw);

    expect(models.main).toBe("anthropic/a");
    expect(models.fallbacks.main).toEqual(["openai/b", "gemini/c"]);
    expect(models.observer).toBe("anthropic/o1");
    expect(models.fallbacks.observer).toEqual(["anthropic/o2"]);
    expect(models.fallbacks.bgSmall).toEqual(["anthropic/s2"]);
    expect(models.overrides.observer?.temperature).toBe("0.2");
  });

  it("has no entry for a role with one model", () => {
    const { models } = parseProvidersToml(raw);
    expect(models.default).toBe("anthropic/d");
    expect(models.fallbacks).not.toHaveProperty("default");
    expect(defaultModels().fallbacks).toEqual({});
  });

  it("writes nothing for a list that wasn't touched", () => {
    const { models } = parseProvidersToml(raw);
    expect(diffProviders([], [], models, structuredClone(models))).toEqual({});
  });

  it("writes the whole list when its first model changes", () => {
    const { models } = parseProvidersToml(raw);
    const current = { ...structuredClone(models), main: "anthropic/new" };
    expect(diffProviders([], [], models, current)).toEqual({
      models: { main: ["anthropic/new", "openai/b", "gemini/c"] },
    });
  });

  it("writes a list with an override as the table's own keys, since $inline holds scalars", () => {
    expect(modelRoleJson("a/x", { temperature: "0.5", thinking: "" }, ["b/y"])).toEqual({
      model: ["a/x", "b/y"],
      temperature: 0.5,
      thinking: null,
    });
    expect(modelRoleJson("a/x", { temperature: "", thinking: "low" }, ["b/y"])).toEqual({
      model: ["a/x", "b/y"],
      temperature: null,
      thinking: "low",
    });
  });

  it("writes a list back as a list once its overrides are cleared", () => {
    expect(modelRoleJson("a/x", { temperature: "", thinking: "" }, ["b/y"])).toEqual([
      "a/x",
      "b/y",
    ]);
  });

  it("clears a role whatever its fallbacks are when its first model is emptied", () => {
    expect(modelRoleJson("", undefined, ["b/y"])).toBeNull();
  });
});

describe("diffProviders", () => {
  it("is empty when nothing changed", () => {
    const models = defaultModels();
    expect(diffProviders([], [], models, models)).toEqual({});
  });

  it("diffs a renamed provider as a delete of the old name plus an add of the new one", () => {
    const models = defaultModels();
    const baseline = [provider({ name: "openai", apiKey: "sk-1" })];
    const current = [provider({ name: "openai-renamed", apiKey: "sk-1" })];
    expect(diffProviders(baseline, current, models, models)).toEqual({
      providers: {
        openai: null,
        "openai-renamed": { type: "anthropic", api_key: "sk-1" },
      },
    });
  });

  it("diffs one changed field on an existing provider without touching siblings", () => {
    const models = defaultModels();
    const baseline = [
      provider({ name: "openai", type: "openai", apiKey: "sk-1", url: "https://a" }),
    ];
    const current = [
      provider({ name: "openai", type: "openai", apiKey: "sk-2", url: "https://a" }),
    ];
    expect(diffProviders(baseline, current, models, models)).toEqual({
      providers: { openai: { api_key: "sk-2" } },
    });
  });

  it("diffs a model role assignment at its background.models path", () => {
    const baseline = defaultModels();
    const current = { ...baseline, bgSmall: "anthropic/claude-haiku" };
    expect(diffProviders([], [], baseline, current)).toEqual({
      background: { models: { small: "anthropic/claude-haiku" } },
    });
  });
});

describe("diffMcpServers", () => {
  it("is empty when nothing changed", () => {
    expect(diffMcpServers([], [])).toEqual({});
  });

  it("diffs one changed field on a stdio server without touching an unrelated http server", () => {
    const baseline = [
      mcpServer({ name: "fs", command: "mcp-fs" }),
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://mcp.example.com/v1",
        headers: { Authorization: "Bearer abc" },
      }),
    ];
    const current = [
      mcpServer({ name: "fs", command: "mcp-fs-v2" }),
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://mcp.example.com/v1",
        headers: { Authorization: "Bearer abc" },
      }),
    ];
    expect(diffMcpServers(baseline, current)).toEqual({
      mcpServers: { fs: { command: "mcp-fs-v2" } },
    });
  });

  it("diffs an http server's url/headers without writing a command field", () => {
    const baseline = [
      mcpServer({ name: "remote", transport: "http", command: "", url: "https://a", headers: {} }),
    ];
    const current = [
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://b",
        headers: { "X-Key": "1" },
      }),
    ];
    const diff = diffMcpServers(baseline, current);
    expect(diff).toEqual({
      mcpServers: { remote: { url: "https://b", headers: { "X-Key": "1" } } },
    });
    expect(diff.mcpServers as Record<string, unknown>).not.toHaveProperty("remote.command");
  });

  it("names each variable or header that was removed, since a patch merges tables", () => {
    const baseline = [
      mcpServer({
        name: "fs",
        env: { A: "1", B: "2", C: "3" },
      }),
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://a",
        headers: { X: "1", Y: "2" },
      }),
    ];
    const current = [
      mcpServer({ name: "fs", env: { A: "1", C: "30", D: "4" } }),
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://a",
        headers: { X: "1" },
      }),
    ];

    expect(diffMcpServers(baseline, current)).toEqual({
      mcpServers: {
        fs: { env: { B: null, C: "30", D: "4" } },
        remote: { headers: { Y: null } },
      },
    });
  });

  it("removes the table when the last variable goes", () => {
    const baseline = [mcpServer({ name: "fs", env: { A: "1" } })];
    const current = [mcpServer({ name: "fs", env: {} })];
    expect(diffMcpServers(baseline, current)).toEqual({ mcpServers: { fs: { env: null } } });
  });

  it("removes a deleted server by name", () => {
    const baseline = [mcpServer({ name: "fs" }), mcpServer({ name: "git" })];
    const current = [mcpServer({ name: "git" })];
    expect(diffMcpServers(baseline, current)).toEqual({ mcpServers: { fs: null } });
  });

  it("adds a new http server as a full entry", () => {
    const current = [
      mcpServer({
        name: "remote",
        transport: "http",
        command: "",
        url: "https://mcp.example.com/v1",
        headers: { Authorization: "Bearer abc" },
      }),
    ];
    expect(diffMcpServers([], current)).toEqual({
      mcpServers: {
        remote: {
          type: "http",
          url: "https://mcp.example.com/v1",
          headers: { Authorization: "Bearer abc" },
        },
      },
    });
  });
});

describe("parseMcpJson HTTP server display", () => {
  it("reads a Claude Desktop style streamable-http server as an http entry", () => {
    const raw = JSON.stringify({
      mcpServers: {
        remote: { type: "streamable-http", url: "https://mcp.example.com/v1", headers: { X: "1" } },
      },
    });
    const [srv] = parseMcpJson(raw);
    expect(srv).toMatchObject({
      name: "remote",
      transport: "http",
      url: "https://mcp.example.com/v1",
      headers: { X: "1" },
      command: "",
    });
  });

  it("falls back to command as the URL when url is absent", () => {
    const raw = JSON.stringify({
      mcpServers: { remote: { transport: "http", command: "http://10.0.0.5:8080/mcp" } },
    });
    const [srv] = parseMcpJson(raw);
    expect(srv?.transport).toBe("http");
    expect(srv?.url).toBe("http://10.0.0.5:8080/mcp");
  });

  it("reads a plain stdio server with no transport field as stdio", () => {
    const raw = JSON.stringify({ mcpServers: { fs: { command: "mcp-fs" } } });
    const [srv] = parseMcpJson(raw);
    expect(srv?.transport).toBe("stdio");
    expect(srv?.command).toBe("mcp-fs");
  });
});

describe("a2a settings diff", () => {
  it("defaults to enabled with no diff emitted for defaults", () => {
    const fields = parseConfigToml("");
    expect(fields.a2a_enabled).toBe(true);
    expect(diffConfigFields(fields, fields)).toEqual({});
  });

  it("round-trips a disabled, private agent with a custom port and public URL", () => {
    const toml =
      '[a2a]\nenabled = false\nport = 7799\npublic_url = "https://example.com/a2a/laptop"\nvisibility = "private"\n';
    const baseline = defaultConfigFields();
    const current = parseConfigToml(toml);

    expect(diffConfigFields(baseline, current)).toEqual({
      a2a: {
        enabled: false,
        port: 7799,
        public_url: "https://example.com/a2a/laptop",
        visibility: "private",
      },
    });
  });

  it("omits the default port and public visibility even when enabled is toggled", () => {
    const baseline = defaultConfigFields();
    const current = { ...baseline, a2a_enabled: false };
    expect(diffConfigFields(baseline, current)).toEqual({ a2a: { enabled: false } });
  });
});

describe("which file a key saves to", () => {
  it("puts the install's keys in the hub's file and the rest in the agent's", () => {
    expect(configFieldOwner(["timezone"])).toBe("hub");
    expect(configFieldOwner(["gateway", "port"])).toBe("hub");
    expect(configFieldOwner(["cloud", "token"])).toBe("hub");
    expect(configFieldOwner(["tracing", "log_level"])).toBe("hub");
    expect(configFieldOwner(["a2a", "port"])).toBe("hub");
    expect(configFieldOwner(["background", "hop_hard_limit"])).toBe("hub");
    expect(configFieldOwner(["a2a", "visibility"])).toBe("agent");
    expect(configFieldOwner(["background", "idle_timeout_spawned_minutes"])).toBe("agent");
    expect(configFieldOwner(["memory", "observer_threshold_tokens"])).toBe("agent");
    expect(configFieldOwner(["webhooks"])).toBe("agent");
  });
});

describe("hub and agent config split", () => {
  it("merges hub-owned and agent-owned keys of a shared section into one form", () => {
    const fields = parseConfigToml(
      '[a2a]\nvisibility = "private"\n[background]\nidle_timeout_spawned_minutes = 7\n',
      'timezone = "Europe/Paris"\n[a2a]\nport = 7799\n[background]\nmax_concurrent = 9\n[gateway]\nport = 9001\n',
    );
    expect(fields.timezone).toBe("Europe/Paris");
    expect(fields.gateway_port).toBe("9001");
    expect(fields.a2a_port).toBe("7799");
    expect(fields.a2a_visibility).toBe("private");
    expect(fields.bg_max_concurrent).toBe("9");
    expect(fields.bg_idle_timeout_spawned_minutes).toBe("7");
  });

  it("routes each changed key to the file that owns it", () => {
    const split = splitConfigPatch({
      timezone: "UTC",
      gateway: { port: 9001 },
      cloud: { enabled: false },
      a2a: { port: 7799, visibility: "private" },
      background: { max_concurrent: 9, hop_hard_limit: 12, subagent_depth_cap: 2 },
      memory: { observer_cooldown_secs: 5 },
      webhooks: { gh: { routing: "inbox" } },
    });
    expect(split.hub).toEqual({
      timezone: "UTC",
      gateway: { port: 9001 },
      cloud: { enabled: false },
      a2a: { port: 7799 },
      background: { max_concurrent: 9, hop_hard_limit: 12 },
    });
    expect(split.agent).toEqual({
      a2a: { visibility: "private" },
      background: { subagent_depth_cap: 2 },
      memory: { observer_cooldown_secs: 5 },
      webhooks: { gh: { routing: "inbox" } },
    });
  });

  it("leaves a shared section out of the file that owns none of its changed keys", () => {
    const split = splitConfigPatch({ a2a: { visibility: "private" } });
    expect(split.hub).toEqual({});
    expect(split.agent).toEqual({ a2a: { visibility: "private" } });
  });
});
