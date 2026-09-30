import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { MOCK_RESIDUUM_VERSION } from "./constants";
import { createMockEnv } from "./env";
import { fetchJson, fetchText, startRouteHarness, type RouteHarness } from "./test-support";
import { updateRoutes } from "./update";

describe("the update routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(updateRoutes, createMockEnv({ deterministic: true }));
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/update${path}`;

  it("knows nothing about updates until a check runs", async () => {
    expect(await fetchJson(url("/status"))).toEqual({
      status: 200,
      body: {
        current: MOCK_RESIDUUM_VERSION,
        latest: null,
        update_available: false,
        last_checked: null,
        checking: false,
        rollback_notice: null,
        unverified_update: null,
      },
    });
  });

  it("records a check, and reports the version as up to date", async () => {
    const res = await fetchJson(url("/check"), { method: "POST" });
    expect(res.body).toMatchObject({
      latest: MOCK_RESIDUUM_VERSION,
      update_available: false,
      last_checked: harness.state.env.clock.iso(),
    });
    expect(((await fetchJson(url("/status"))).body as { last_checked: string }).last_checked).toBe(
      harness.state.env.clock.iso(),
    );
  });

  it("refuses to apply before a check found a version", async () => {
    expect(await fetchText(url("/apply"), { method: "POST" })).toEqual({
      status: 400,
      body: "no update version known — run a check first",
    });
    await fetchJson(url("/check"), { method: "POST" });
    expect((await fetchJson(url("/apply"), { method: "POST" })).status).toBe(200);
  });
});
