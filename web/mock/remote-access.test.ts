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
});
