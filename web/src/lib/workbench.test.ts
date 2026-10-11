import { describe, expect, it } from "vitest";
import { artifactUrl, NO_SECURE_ORIGIN_REASON, resolveArtifactsOrigin } from "./workbench";
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

  it("does not invent a :port HTTPS origin on Residuum Cloud while the relay hasn't announced one", () => {
    const cloud = {
      origin: "https://bear.agent-residuum.com",
      protocol: "https:",
      hostname: "bear.agent-residuum.com",
    };
    const result = resolveArtifactsOrigin(info({ relay: null }), cloud);
    expect(result).toEqual({ ok: false, reason: NO_SECURE_ORIGIN_REASON });
  });

  it("does not point an HTTPS reverse proxy at the plain-HTTP artifacts port", () => {
    const proxied = {
      origin: "https://workbench.example.com",
      protocol: "https:",
      hostname: "workbench.example.com",
    };
    expect(resolveArtifactsOrigin(info({ relay: null }), proxied)).toEqual({
      ok: false,
      reason: NO_SECURE_ORIGIN_REASON,
    });
    // A relay announcing another UI origin doesn't make this one its UI.
    expect(resolveArtifactsOrigin(info(), proxied)).toEqual({
      ok: false,
      reason: NO_SECURE_ORIGIN_REASON,
    });
  });

  it("still surfaces the listener's own unavailable reason over HTTPS", () => {
    const result = resolveArtifactsOrigin(
      info({ port: null, relay: null, unavailable_reason: "port 7702 is in use" }),
      relayed,
    );
    expect(result).toEqual({ ok: false, reason: "port 7702 is in use" });
  });
});

describe("artifactUrl", () => {
  it("ends in a slash so relative URLs resolve inside the artifact", () => {
    expect(artifactUrl("http://localhost:7702", "wiki-graph")).toBe(
      "http://localhost:7702/wiki-graph/",
    );
  });
});
