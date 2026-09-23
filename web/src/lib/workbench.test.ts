import { describe, expect, it } from "vitest";
import { resolveArtifactsOrigin, artifactUrl } from "./workbench";
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
    artifacts_origin: "https://bear.workbench.agent-residuum.com",
  },
  ...overrides,
});

describe("resolveArtifactsOrigin", () => {
  it("uses the artifacts port on this host locally", () => {
    expect(resolveArtifactsOrigin(info(), local)).toEqual({
      ok: true,
      origin: "http://localhost:7702",
    });
  });

  it("keeps the host the UI was reached on, such as a LAN address", () => {
    const lan = { origin: "http://192.168.1.5:7700", protocol: "http:", hostname: "192.168.1.5" };
    expect(resolveArtifactsOrigin(info({ relay: null }), lan)).toEqual({
      ok: true,
      origin: "http://192.168.1.5:7702",
    });
  });

  it("uses the relay's artifacts origin when viewed through the relay", () => {
    expect(resolveArtifactsOrigin(info(), relayed)).toEqual({
      ok: true,
      origin: "https://bear.workbench.agent-residuum.com",
    });
  });

  it("reports why artifacts are unavailable", () => {
    expect(
      resolveArtifactsOrigin(
        info({ port: null, unavailable_reason: "port 7702 is in use" }),
        local,
      ),
    ).toEqual({ ok: false, reason: "port 7702 is in use" });
  });

  it("explains a missing relay origin rather than pointing at an unreachable port", () => {
    const result = resolveArtifactsOrigin(info({ port: null, relay: null }), relayed);
    expect(result.ok).toBe(false);
  });
});

describe("artifactUrl", () => {
  it("ends in a slash so relative URLs resolve inside the artifact", () => {
    expect(artifactUrl("http://localhost:7702", "wiki-graph")).toBe(
      "http://localhost:7702/wiki-graph/",
    );
  });
});
