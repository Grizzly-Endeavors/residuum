import { describe, expect, it } from "vitest";
import {
  defaultConfigFields,
  defaultModels,
  diffConfigFields,
  diffMcpServers,
  diffProviders,
  modelRoleJson,
  parseConfigToml,
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
