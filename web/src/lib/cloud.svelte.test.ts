import { describe, expect, it } from "vitest";
import { connectTarget, phaseOf } from "./cloud.svelte";
import type { CloudStatusResponse } from "./types";

function status(overrides: Partial<CloudStatusResponse> = {}): CloudStatusResponse {
  return {
    status: "disconnected",
    user_id: null,
    has_token: false,
    enabled: false,
    viewed_via_tunnel: false,
    ...overrides,
  };
}

describe("connectTarget", () => {
  it("signs in at Residuum Cloud's relay when no relay is set", () => {
    expect(connectTarget("", "")).toEqual({
      url: "https://agent-residuum.com/connect?port=7700",
      host: "agent-residuum.com",
    });
  });

  it("follows the relay URL, so a relay on this machine is signed in to there", () => {
    expect(connectTarget("ws://127.0.0.1:8080/tunnel/register", "7701")).toEqual({
      url: "http://127.0.0.1:8080/connect?port=7701",
      host: "127.0.0.1:8080",
    });
    expect(connectTarget(" wss://relay.example.com/tunnel/register ", "7700")?.url).toBe(
      "https://relay.example.com/connect?port=7700",
    );
  });

  it("falls back to the default gateway port when the field is empty or blank", () => {
    expect(connectTarget("", "  ")?.url).toContain("port=7700");
  });

  it("has nowhere to sign in for an address that isn't a ws or wss one", () => {
    expect(connectTarget("not-a-url", "7700")).toBeNull();
    expect(connectTarget("https://relay.example.com", "7700")).toBeNull();
    expect(connectTarget("ws://", "7700")).toBeNull();
  });
});

describe("phaseOf", () => {
  it("tells the four states apart", () => {
    expect(phaseOf(status({ status: "connected", has_token: true, enabled: true }))).toBe(
      "connected",
    );
    expect(phaseOf(status({ status: "connecting", has_token: true, enabled: true }))).toBe(
      "connecting",
    );
    expect(phaseOf(status({ has_token: true, enabled: false }))).toBe("disconnected");
    expect(phaseOf(status())).toBe("none");
  });

  it("counts a saved token that is switched on as connecting while the tunnel isn't up yet", () => {
    expect(phaseOf(status({ has_token: true, enabled: true }))).toBe("connecting");
  });
});
