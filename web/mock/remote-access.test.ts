import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { RemoteAccessStatus } from "../src/lib/generated/RemoteAccessStatus";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

describe("the remote access routes", () => {
  let harness: MockServerHarness;

  const call = (
    method: string,
    path: string,
    body?: unknown,
  ): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}${path}`, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
    });

  /** The status code of a request whose answer has no body. */
  const statusOf = async (method: string, path: string, body?: unknown): Promise<number> => {
    const res = await fetch(`${harness.baseUrl}${path}`, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }),
    });
    return res.status;
  };

  beforeEach(async () => {
    harness = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await harness.close();
  });

  it("reports the older tunnel by default", async () => {
    const answer = await call("GET", "/api/hub/remote-access/status");
    expect(answer.status).toBe(200);
    const status = answer.body as RemoteAccessStatus;
    expect(status.state).toBe("legacy");
    expect(status.hosts?.ui).toBe("mock-user.agent-residuum.com");
    expect(status.recovery_code).toBeNull();
  });

  it("shows a recovery code after a reset until it is saved", async () => {
    expect(
      await statusOf("POST", "/api/hub/remote-access/reset-pins", { recovery_code: "short" }),
    ).toBe(400);
    expect(
      await statusOf("POST", "/api/hub/remote-access/reset-pins", {
        recovery_code: "ABCDEFGHIJKLMNOPQRST",
      }),
    ).toBe(204);
    const pending = (await call("GET", "/api/hub/remote-access/status")).body as RemoteAccessStatus;
    expect(pending.recovery_code_pending).toBe(true);
    expect(pending.recovery_code).toHaveLength(20);
    expect(await statusOf("POST", "/api/hub/remote-access/recovery-code/saved")).toBe(204);
    const saved = (await call("GET", "/api/hub/remote-access/status")).body as RemoteAccessStatus;
    expect(saved.recovery_code).toBeNull();
  });

  it("has no other instances on the older tunnel", async () => {
    const status = (await call("GET", "/api/hub/remote-access/status")).body as RemoteAccessStatus;
    expect(status.instances).toEqual([]);
    expect(status.pending_joins).toEqual([]);
    expect(status.join).toBeNull();
    expect(status.pins.every((pin) => !pin.removable)).toBe(true);
  });

  describe("with other instances", () => {
    const read = async (): Promise<RemoteAccessStatus> =>
      (await call("GET", "/api/hub/remote-access/status")).body as RemoteAccessStatus;

    beforeEach(async () => {
      expect(await statusOf("POST", "/api/mock/remote-access", { scenario: "cluster" })).toBe(204);
    });

    it("lists two instances, a join in progress and one waiting for approval", async () => {
      const status = await read();
      expect(status.instances.map((instance) => instance.slug)).toEqual(["laptop", "desktop"]);
      expect(status.join?.state).toBe("waiting");
      expect(status.join?.code).toHaveLength(6);
      expect(status.pending_joins).toHaveLength(1);
      expect(status.pins.filter((pin) => pin.removable)).toHaveLength(1);
    });

    it("starts a join", async () => {
      expect(await statusOf("POST", "/api/hub/remote-access/join", { instance: "" })).toBe(400);
      expect(await statusOf("POST", "/api/hub/remote-access/join", { instance: "tower" })).toBe(
        204,
      );
      expect((await read()).join?.instance).toBe("tower");
    });

    it("answers a join request once", async () => {
      expect(await statusOf("POST", "/api/hub/remote-access/joins/join-1/approve")).toBe(204);
      expect((await read()).pending_joins).toEqual([]);
      expect(await statusOf("POST", "/api/hub/remote-access/joins/join-1/deny")).toBe(404);
    });

    it("removes only a removable certificate account", async () => {
      const own = "https://acme-v02.api.letsencrypt.org/acme/acct/1";
      const gone = "https://acme-v02.api.letsencrypt.org/acme/acct/3";
      expect(
        await statusOf("POST", "/api/hub/remote-access/pins/remove", { account_uri: own }),
      ).toBe(400);
      expect(
        await statusOf("POST", "/api/hub/remote-access/pins/remove", { account_uri: gone }),
      ).toBe(204);
      expect((await read()).pins.map((pin) => pin.account_uri)).not.toContain(gone);
    });

    it("moves the active instance", async () => {
      expect(await statusOf("POST", "/api/hub/remote-access/instances/nowhere/activate")).toBe(404);
      expect(await statusOf("POST", "/api/hub/remote-access/instances/laptop/activate")).toBe(204);
      const active = (await read()).instances.filter((instance) => instance.active);
      expect(active.map((instance) => instance.slug)).toEqual(["laptop"]);
    });
  });
});
