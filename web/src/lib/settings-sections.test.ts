import { describe, expect, it } from "vitest";
import {
  AGENT_SECTIONS,
  ALL_SECTIONS,
  defaultSection,
  isSectionOf,
  scopeKind,
  sectionAfterScopeSwitch,
  sectionForConfigKey,
  sectionFromOldName,
  sectionGroups,
  sectionsOf,
} from "./settings-sections";

const ids = (kind: "agent" | "all"): string[] => sectionsOf(kind).map((s) => s.id);

describe("section registry", () => {
  it("lists an agent's sections in order, with the Advanced group last", () => {
    expect(AGENT_SECTIONS.map((s) => [s.id, s.label, s.group])).toEqual([
      ["model", "Model", "main"],
      ["connections", "Connections", "main"],
      ["tools", "Tools & skills", "main"],
      ["memory", "Memory", "main"],
      ["schedule", "Schedule", "main"],
      ["runtime", "Runtime", "advanced"],
      ["servers", "Tool servers", "advanced"],
      ["a2a", "Agent-to-agent", "advanced"],
      ["raw", "Raw config", "advanced"],
      ["history", "History", "advanced"],
    ]);
  });

  it("lists the install-wide sections in order", () => {
    expect(ALL_SECTIONS.map((s) => [s.id, s.label, s.group])).toEqual([
      ["general", "General", "main"],
      ["notifications", "Notifications", "main"],
      ["cloud", "Residuum Cloud", "main"],
      ["keys", "Saved keys", "main"],
      ["updates", "Updates", "main"],
      ["limits", "Session limits", "main"],
      ["listener", "Agent-to-agent", "advanced"],
      ["diagnostics", "Diagnostics", "advanced"],
      ["raw", "Raw config", "advanced"],
      ["history", "History", "advanced"],
    ]);
  });

  it("shares only the raw editors and history between scopes", () => {
    const shared = ids("agent").filter((id) => ids("all").includes(id));
    expect(shared.sort()).toEqual(["history", "raw"]);
  });

  it("splits each scope into the main list and the Advanced group", () => {
    const groups = sectionGroups("agent");
    expect(groups.main.map((s) => s.id)).toEqual([
      "model",
      "connections",
      "tools",
      "memory",
      "schedule",
    ]);
    expect(groups.advanced.map((s) => s.id)).toEqual([
      "runtime",
      "servers",
      "a2a",
      "raw",
      "history",
    ]);
  });

  it("reads `_all` as the install and anything else as an agent", () => {
    expect(scopeKind("_all")).toBe("all");
    expect(scopeKind("scout")).toBe("agent");
  });

  it("opens an agent on Model and the install on General", () => {
    expect(defaultSection("agent")).toBe("model");
    expect(defaultSection("all")).toBe("general");
    expect(isSectionOf("agent", defaultSection("agent"))).toBe(true);
    expect(isSectionOf("all", defaultSection("all"))).toBe(true);
  });

  it("knows which scope a section belongs to", () => {
    expect(isSectionOf("agent", "model")).toBe(true);
    expect(isSectionOf("all", "model")).toBe(false);
    expect(isSectionOf("all", "listener")).toBe(true);
    expect(isSectionOf("agent", "listener")).toBe(false);
    expect(isSectionOf("agent", "providers")).toBe(false);
  });
});

describe("switching scope", () => {
  it("keeps the section when the new scope has it", () => {
    expect(sectionAfterScopeSwitch("raw", "all")).toBe("raw");
    expect(sectionAfterScopeSwitch("history", "agent")).toBe("history");
  });

  it("falls back to the new scope's default when it doesn't", () => {
    expect(sectionAfterScopeSwitch("memory", "all")).toBe("general");
    expect(sectionAfterScopeSwitch("cloud", "agent")).toBe("model");
  });

  it("stays on the section list when no section was open", () => {
    expect(sectionAfterScopeSwitch(null, "all")).toBeNull();
  });
});

describe("old section names", () => {
  it.each([
    ["runtime", "agent", "runtime"],
    ["providers", "agent", "model"],
    ["channels", "agent", "connections"],
    ["integrations", "agent", "connections"],
    ["webhooks", "agent", "connections"],
    ["pulses", "agent", "schedule"],
    ["memory", "agent", "memory"],
    ["skills", "agent", "tools"],
    ["mcp", "agent", "servers"],
    ["a2a", "agent", "a2a"],
    ["history", "agent", "history"],
  ] as const)("sends agent %s to %s of the agent", (old, scope, section) => {
    expect(sectionFromOldName(old, "agent")).toEqual({ scope, section });
  });

  it.each([
    ["general", "general"],
    ["cloud", "cloud"],
    ["a2a", "listener"],
    ["sessions", "limits"],
    ["tracing", "diagnostics"],
    ["update", "updates"],
    ["secrets", "keys"],
    ["agent-keys", "keys"],
    ["history", "history"],
  ] as const)("sends hub %s to %s of All agents", (old, section) => {
    expect(sectionFromOldName(old, "hub")).toEqual({ scope: "all", section });
  });

  it("moves a hub section named under an agent to All agents", () => {
    expect(sectionFromOldName("general", "agent")).toEqual({ scope: "all", section: "general" });
    expect(sectionFromOldName("tracing", "agent")).toEqual({
      scope: "all",
      section: "diagnostics",
    });
    expect(sectionFromOldName("agent-keys", "agent")).toEqual({ scope: "all", section: "keys" });
  });

  it("moves an agent section named under the hub to the agent", () => {
    expect(sectionFromOldName("providers", "hub")).toEqual({ scope: "agent", section: "model" });
    expect(sectionFromOldName("runtime", "hub")).toEqual({ scope: "agent", section: "runtime" });
  });

  it("keeps a name both scopes had in the scope the URL named", () => {
    expect(sectionFromOldName("a2a", "hub")?.scope).toBe("all");
    expect(sectionFromOldName("a2a", "agent")?.scope).toBe("agent");
    expect(sectionFromOldName("history", "hub")?.scope).toBe("all");
    expect(sectionFromOldName("history", "agent")?.scope).toBe("agent");
  });

  it("doesn't know a made-up name, or one from the object prototype", () => {
    expect(sectionFromOldName("nope", "agent")).toBeNull();
    expect(sectionFromOldName("constructor", "hub")).toBeNull();
  });
});

describe("config keys", () => {
  it("maps an agent's providers.toml to Model", () => {
    for (const key of [
      "providers.anthropic.api_key",
      "models.main",
      "models",
      "background.small",
    ]) {
      expect(sectionForConfigKey("agent", "providers", key)).toBe("model");
    }
  });

  it("maps mcp.json to Tool servers", () => {
    expect(sectionForConfigKey("agent", "mcp", "mcpServers.files.command")).toBe("servers");
  });

  it.each([
    ["temperature", "model"],
    ["thinking", "model"],
    ["discord.token", "connections"],
    ["telegram", "connections"],
    ["teams.port", "connections"],
    ["webhooks.alerts.secret", "connections"],
    ["skills.dirs", "tools"],
    ["tools.path", "tools"],
    ["web_search.backend", "tools"],
    ["memory.observer_threshold_tokens", "memory"],
    ["subconscious.enabled", "memory"],
    ["learning.enabled", "memory"],
    ["pulse.enabled", "schedule"],
    ["background.idle_timeout_spawned_minutes", "schedule"],
    ["timeout_secs", "runtime"],
    ["max_tokens", "runtime"],
    ["retry.max_retries", "runtime"],
    ["agent.max_tool_iterations", "runtime"],
    ["idle.timeout_minutes", "runtime"],
    ["a2a.visibility", "a2a"],
  ] as const)("maps an agent's config.toml key %s to %s", (key, section) => {
    expect(sectionForConfigKey("agent", "config", key)).toBe(section);
  });

  it("reads the top-level key out of a dotted or indexed path", () => {
    expect(sectionForConfigKey("agent", "config", "webhooks[0].name")).toBe("connections");
  });

  it.each([
    ["timezone", "general"],
    ["gateway.port", "general"],
    ["push.contact", "notifications"],
    ["cloud.relay_url", "cloud"],
    ["background.max_concurrent", "limits"],
    ["a2a.port", "listener"],
    ["tracing.log_level", "diagnostics"],
  ] as const)("maps the hub's config.toml key %s to %s", (key, section) => {
    expect(sectionForConfigKey("all", "config", key)).toBe(section);
  });

  it("has no section for a key no form edits, so the diagnostic leads to Raw config", () => {
    expect(sectionForConfigKey("agent", "config", "autostart")).toBeNull();
    expect(sectionForConfigKey("agent", "config", "made_up_key")).toBeNull();
    expect(sectionForConfigKey("agent", "config", "constructor")).toBeNull();
    expect(sectionForConfigKey("agent", "config", "")).toBeNull();
    expect(sectionForConfigKey("all", "providers", "providers.x")).toBeNull();
  });
});
