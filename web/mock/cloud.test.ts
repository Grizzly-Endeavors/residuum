import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fetchJson, fetchText, startMockServer, type MockServerHarness } from "./test-support";

describe("the Residuum Cloud routes", () => {
  let harness: MockServerHarness;

  const call = (method: string, path: string, body?: unknown): ReturnType<typeof fetchJson> =>
    fetchJson(`${harness.baseUrl}${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  const status = async (): Promise<unknown> => (await call("GET", "/api/hub/cloud/status")).body;

  beforeEach(async () => {
    harness = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await harness.close();
  });

  it("starts with no account: not connected, no token", async () => {
    expect(await status()).toEqual({
      status: "disconnected",
      user_id: null,
      has_token: false,
      enabled: false,
      viewed_via_tunnel: false,
    });
  });

  it("connects when the relay hands a token back, and keeps it as a secret", async () => {
    expect((await call("POST", "/api/mock/cloud-callback", { token: "rst_one" })).status).toBe(200);

    expect(await status()).toMatchObject({
      status: "connected",
      user_id: "mock-user",
      has_token: true,
      enabled: true,
    });
    expect(harness.hub.hubState.secrets.get("cloud_token")).toBe("rst_one");
    expect(harness.hub.hubState.hubConfigToml).toContain('token = "secret:cloud_token"');
  });

  it("reads the account from the hub's config, so a settings write changes it", async () => {
    await call("PATCH", "/api/hub/config/patch", { cloud: { enabled: true, token: "secret:x" } });
    expect(await status()).toMatchObject({ status: "connected", has_token: true, enabled: true });

    await call("PATCH", "/api/hub/config/patch", { cloud: { token: null } });
    expect(await status()).toMatchObject({ status: "disconnected", has_token: false });
  });

  it("disconnects by switching the tunnel off and keeping the token", async () => {
    await call("POST", "/api/mock/cloud-callback");

    expect((await call("POST", "/api/hub/cloud/disconnect")).status).toBe(200);

    expect(await status()).toMatchObject({
      status: "disconnected",
      user_id: null,
      has_token: true,
      enabled: false,
    });
  });

  it("reports the status inside the hub's status too", async () => {
    await call("POST", "/api/mock/cloud-callback");
    const hubStatus = (await call("GET", "/api/hub/status")).body as { tunnel: unknown };
    expect(hubStatus.tunnel).toMatchObject({ status: "connected", has_token: true });
  });

  it("holds the tunnel at a state a test names, until the config is changed by disconnecting", async () => {
    await call("POST", "/api/mock/cloud-callback");
    expect((await call("POST", "/api/mock/cloud", { tunnel: "connecting" })).status).toBe(200);
    expect(await status()).toMatchObject({ status: "connecting", user_id: null });

    await call("POST", "/api/hub/cloud/disconnect");
    expect(await status()).toMatchObject({ status: "disconnected" });
  });

  it("refuses to disconnect for a status read through the tunnel, as the gateway's guard does", async () => {
    await call("POST", "/api/mock/cloud-callback");
    await call("POST", "/api/mock/cloud", { via_tunnel: true });
    expect(await status()).toMatchObject({ viewed_via_tunnel: true });

    const refused = await fetchText(`${harness.baseUrl}/api/hub/cloud/disconnect`, {
      method: "POST",
    });

    expect(refused.status).toBe(403);
    expect(refused.body).toContain("can't be done remotely");
    expect(await status()).toMatchObject({ status: "connected", enabled: true });
  });

  it("refuses a control it can't read", async () => {
    expect((await call("POST", "/api/mock/cloud", { tunnel: "sideways" })).status).toBe(422);
    expect((await call("POST", "/api/mock/cloud", { via_tunnel: "yes" })).status).toBe(422);
    expect((await call("POST", "/api/mock/cloud-callback", { token: "" })).status).toBe(422);
  });

  it("starts over on reset", async () => {
    await call("POST", "/api/mock/cloud-callback");
    await call("POST", "/api/mock/cloud", { via_tunnel: true });

    await call("POST", "/api/mock/reset");

    expect(await status()).toMatchObject({
      status: "disconnected",
      has_token: false,
      viewed_via_tunnel: false,
    });
  });
});
