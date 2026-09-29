import { describe, expect, it } from "vitest";
import {
  AGENT_SECTIONS,
  HUB_SECTIONS,
  defaultSection,
  isSectionOf,
  locateSection,
  sectionsFor,
} from "./settings-sections";

const ids = (scope: "hub" | "agent"): string[] => sectionsFor(scope).map((s) => s.id);

describe("settings sections", () => {
  it("keeps hub settings to the hub's own sections", () => {
    expect(ids("hub")).toEqual([
      "general",
      "cloud",
      "a2a",
      "sessions",
      "tracing",
      "update",
      "secrets",
      "agent-keys",
      "history",
    ]);
  });

  it("keeps agent settings to the agent's own sections", () => {
    expect(ids("agent")).toEqual([
      "runtime",
      "providers",
      "channels",
      "pulses",
      "memory",
      "skills",
      "mcp",
      "a2a",
      "webhooks",
      "history",
    ]);
  });

  it("shares no section between scopes except A2A and History, which each scope shows its own half of", () => {
    const shared = HUB_SECTIONS.map((s) => s.id).filter((id) =>
      AGENT_SECTIONS.some((a) => a.id === id),
    );
    expect(shared.sort()).toEqual(["a2a", "history"]);
  });

  it("opens each scope on a section it has", () => {
    expect(isSectionOf("hub", defaultSection("hub"))).toBe(true);
    expect(isSectionOf("agent", defaultSection("agent"))).toBe(true);
  });

  it("finds a section in the other scope when the URL named the wrong one", () => {
    expect(locateSection("secrets", "agent")).toEqual({ scope: "hub", section: "secrets" });
    expect(locateSection("providers", "hub")).toEqual({ scope: "agent", section: "providers" });
  });

  it("prefers the scope the URL named when both have the section", () => {
    expect(locateSection("a2a", "hub")).toEqual({ scope: "hub", section: "a2a" });
    expect(locateSection("a2a", "agent")).toEqual({ scope: "agent", section: "a2a" });
  });

  it("follows sections that moved or were renamed", () => {
    expect(locateSection("agent-keys", "agent")).toEqual({ scope: "hub", section: "agent-keys" });
    expect(locateSection("integrations", "hub")).toEqual({ scope: "agent", section: "channels" });
  });

  it("does not know a made-up section", () => {
    expect(locateSection("nope", "agent")).toBeNull();
  });
});
