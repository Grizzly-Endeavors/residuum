import { describe, expect, it } from "vitest";
import {
  defaultModels,
  parseConfigToml,
  serializeConfigToml,
  serializeMcpJson,
  serializeProvidersToml,
} from "./settings-toml";
import type { McpServerEntry, SettingsProviderEntry } from "./types";

function provider(overrides: Partial<SettingsProviderEntry> = {}): SettingsProviderEntry {
  return { name: "", type: "anthropic", apiKey: "", url: "", keepAlive: "", ...overrides };
}

function mcpServer(overrides: Partial<McpServerEntry> = {}): McpServerEntry {
  return { name: "", command: "npx", args: [], env: {}, ...overrides };
}

describe("serializeProvidersToml", () => {
  it("omits a provider with no name", () => {
    const toml = serializeProvidersToml([provider({ name: "" })], defaultModels());
    expect(toml).not.toContain("[providers.");
  });

  it("omits a provider with a whitespace-only name", () => {
    const toml = serializeProvidersToml([provider({ name: "   " })], defaultModels());
    expect(toml).not.toContain("[providers.");
  });

  it("trims surrounding whitespace from a named provider", () => {
    const toml = serializeProvidersToml([provider({ name: "  openai  " })], defaultModels());
    expect(toml).toContain("[providers.openai]");
  });
});

describe("serializeMcpJson", () => {
  it("omits an mcp server with no name", () => {
    const json = JSON.parse(serializeMcpJson([mcpServer({ name: "" })])) as {
      mcpServers: Record<string, unknown>;
    };
    expect(Object.keys(json.mcpServers)).toHaveLength(0);
  });

  it("omits an mcp server with a whitespace-only name", () => {
    const json = JSON.parse(serializeMcpJson([mcpServer({ name: "   " })])) as {
      mcpServers: Record<string, unknown>;
    };
    expect(Object.keys(json.mcpServers)).toHaveLength(0);
  });

  it("trims surrounding whitespace from a named mcp server", () => {
    const json = JSON.parse(serializeMcpJson([mcpServer({ name: "  fetch  " })])) as {
      mcpServers: Record<string, unknown>;
    };
    expect(Object.keys(json.mcpServers)).toEqual(["fetch"]);
  });
});

describe("background idle timeouts", () => {
  it("round-trips the artifact session idle timeout through config.toml", () => {
    const fields = parseConfigToml("[background]\nidle_timeout_artifact_minutes = 25\n");
    expect(fields.bg_idle_timeout_artifact_minutes).toBe("25");
    const out = serializeConfigToml(fields);
    expect(out).toContain("[background]");
    expect(out).toContain("idle_timeout_artifact_minutes = 25");
  });
});

describe("a2a settings", () => {
  it("defaults to enabled with no section emitted", () => {
    const fields = parseConfigToml("");
    expect(fields.a2a_enabled).toBe(true);
    expect(fields.a2a_port).toBe("");
    expect(fields.a2a_visibility).toBe("");
    expect(serializeConfigToml(fields)).not.toContain("[a2a]");
  });

  it("round-trips a disabled, private agent with a custom port and public URL", () => {
    const toml =
      '[a2a]\nenabled = false\nport = 7799\npublic_url = "https://example.com/a2a/laptop"\nvisibility = "private"\n';
    const fields = parseConfigToml(toml);
    expect(fields.a2a_enabled).toBe(false);
    expect(fields.a2a_port).toBe("7799");
    expect(fields.a2a_public_url).toBe("https://example.com/a2a/laptop");
    expect(fields.a2a_visibility).toBe("private");

    const out = serializeConfigToml(fields);
    expect(out).toContain("[a2a]");
    expect(out).toContain("enabled = false");
    expect(out).toContain("port = 7799");
    expect(out).toContain('public_url = "https://example.com/a2a/laptop"');
    expect(out).toContain('visibility = "private"');
  });

  it("omits the default port and public visibility even when the section is otherwise emitted", () => {
    const fields = parseConfigToml("");
    fields.a2a_enabled = false;
    const out = serializeConfigToml(fields);
    expect(out).toContain("[a2a]");
    expect(out).not.toContain("port = 7702");
    expect(out).not.toContain('visibility = "public"');
  });
});
