import { describe, expect, it } from "vitest";
import { defaultModels, serializeMcpJson, serializeProvidersToml } from "./settings-toml";
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
