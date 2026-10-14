import { describe, expect, it } from "vitest";
import { accountTarget, phaseOf, signInTarget, teamsMessagingEndpoint } from "./cloud.svelte";
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

describe("teamsMessagingEndpoint", () => {
  it("builds the address pasted into the Teams developer portal on the instance's own origin", () => {
    const source = { instance_origin: "https://laptop.bear.agent-residuum.com" };
    expect(teamsMessagingEndpoint(source, "scout")).toBe(
      "https://laptop.bear.agent-residuum.com/teams/scout",
    );
    expect(
      teamsMessagingEndpoint(
        { instance_origin: "https://laptop.bear.agent-residuum.com/" },
        "scout",
      ),
    ).toBe("https://laptop.bear.agent-residuum.com/teams/scout");
  });

  it("has no address until the status names an instance origin", () => {
    expect(teamsMessagingEndpoint({}, "scout")).toBeNull();
    expect(teamsMessagingEndpoint({ instance_origin: "" }, "scout")).toBeNull();
  });
});

describe("accountTarget", () => {
  it("manages the account on Residuum Cloud's dashboard when no relay is set", () => {
    expect(accountTarget("")).toEqual({
      url: "https://agent-residuum.com/dashboard",
      host: "agent-residuum.com",
    });
  });

  it("follows the relay URL", () => {
    expect(accountTarget("ws://127.0.0.1:8080/tunnel/v2/register")?.url).toBe(
      "http://127.0.0.1:8080/dashboard",
    );
  });

  it("has nowhere to go for an address that isn't a ws or wss one", () => {
    expect(accountTarget("https://relay.example.com")).toBeNull();
  });
});

describe("signInTarget", () => {
  it("signs in at Residuum Cloud's relay when no relay is set", () => {
    expect(signInTarget("", "")).toEqual({
      relayUrl: "",
      port: "7700",
      host: "agent-residuum.com",
    });
  });

  it("follows the relay URL, so a relay on this machine is signed in to there", () => {
    expect(signInTarget("ws://127.0.0.1:8080/tunnel/v2/register", "7701")).toEqual({
      relayUrl: "ws://127.0.0.1:8080/tunnel/v2/register",
      port: "7701",
      host: "127.0.0.1:8080",
    });
    expect(signInTarget(" wss://relay.example.com/tunnel/v2/register ", "7700")).toEqual({
      relayUrl: "wss://relay.example.com/tunnel/v2/register",
      port: "7700",
      host: "relay.example.com",
    });
  });

  it("falls back to the default gateway port when the field is empty or blank", () => {
    expect(signInTarget("", "  ")?.port).toBe("7700");
  });

  it("has nowhere to sign in for an address that isn't a ws or wss one", () => {
    expect(signInTarget("not-a-url", "7700")).toBeNull();
    expect(signInTarget("https://relay.example.com", "7700")).toBeNull();
    expect(signInTarget("ws://", "7700")).toBeNull();
  });
});

describe("phaseOf", () => {
  it("tells the five states apart", () => {
    expect(phaseOf(status({ status: "connected", has_token: true, enabled: true }))).toBe(
      "connected",
    );
    expect(phaseOf(status({ status: "connecting", has_token: true, enabled: true }))).toBe(
      "connecting",
    );
    expect(phaseOf(status({ status: "refused", has_token: true, enabled: true }))).toBe("refused");
    expect(phaseOf(status({ has_token: true, enabled: false }))).toBe("disconnected");
    expect(phaseOf(status())).toBe("none");
  });

  it("counts a saved token that is switched on as connecting while the tunnel isn't up yet", () => {
    expect(phaseOf(status({ has_token: true, enabled: true }))).toBe("connecting");
  });

  it("reads a refusal the tunnel left behind as switched off once the token is off or gone", () => {
    expect(phaseOf(status({ status: "refused", has_token: true, enabled: false }))).toBe(
      "disconnected",
    );
    expect(phaseOf(status({ status: "refused" }))).toBe("none");
  });
});
