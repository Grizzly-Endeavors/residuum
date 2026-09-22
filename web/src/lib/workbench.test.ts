import { describe, expect, it } from "vitest";
import { resolveToolsOrigin, toolUrl } from "./workbench";
import type { WorkbenchInfo } from "./types";

const local = { origin: "http://localhost:7700", protocol: "http:", hostname: "localhost" };
const relayed = {
  origin: "https://bear.agent-residuum.com",
  protocol: "https:",
  hostname: "bear.agent-residuum.com",
};

const info = (overrides: Partial<WorkbenchInfo> = {}): WorkbenchInfo => ({
  port: 7702,
  unavailable_reason: null,
  relay: {
    ui_origin: "https://bear.agent-residuum.com",
    tools_origin: "https://bear.workbench.agent-residuum.com",
  },
  ...overrides,
});

describe("resolveToolsOrigin", () => {
  it("uses the tools port on this host locally", () => {
    expect(resolveToolsOrigin(info(), local)).toEqual({
      ok: true,
      origin: "http://localhost:7702",
    });
  });

  it("keeps the host the UI was reached on, such as a LAN address", () => {
    const lan = { origin: "http://192.168.1.5:7700", protocol: "http:", hostname: "192.168.1.5" };
    expect(resolveToolsOrigin(info({ relay: null }), lan)).toEqual({
      ok: true,
      origin: "http://192.168.1.5:7702",
    });
  });

  it("uses the relay's tools origin when viewed through the relay", () => {
    expect(resolveToolsOrigin(info(), relayed)).toEqual({
      ok: true,
      origin: "https://bear.workbench.agent-residuum.com",
    });
  });

  it("reports why tools are unavailable", () => {
    expect(
      resolveToolsOrigin(info({ port: null, unavailable_reason: "port 7702 is in use" }), local),
    ).toEqual({ ok: false, reason: "port 7702 is in use" });
  });

  it("explains a missing relay origin rather than pointing at an unreachable port", () => {
    const result = resolveToolsOrigin(info({ port: null, relay: null }), relayed);
    expect(result.ok).toBe(false);
  });
});

describe("toolUrl", () => {
  it("ends in a slash so relative URLs resolve inside the tool", () => {
    expect(toolUrl("http://localhost:7702", "wiki-graph")).toBe(
      "http://localhost:7702/wiki-graph/",
    );
  });
});
