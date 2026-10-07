import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { DeviceListResponse } from "../src/lib/generated/DeviceListResponse";
import type { PairLinkResponse } from "../src/lib/generated/PairLinkResponse";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

describe("the pairing routes", () => {
  let harness: MockServerHarness;

  const call = (method: string, path: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}${path}`, { method });

  beforeEach(async () => {
    harness = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await harness.close();
  });

  it("serves locally, where no browser needs to pair", async () => {
    expect(await call("GET", "/api/hub/pairing/state")).toEqual({
      status: 200,
      body: { remote: false, paired: true },
    });
  });

  it("shows the recovery codes with the first pairing link only", async () => {
    const first = (await call("POST", "/api/hub/remote-access/pair-link")).body as PairLinkResponse;
    expect(first.link).toContain("/pair#token=");
    expect(first.recovery_codes).toHaveLength(10);
    const second = (await call("POST", "/api/hub/remote-access/pair-link"))
      .body as PairLinkResponse;
    expect(second.recovery_codes).toBeNull();
    const listing = (await call("GET", "/api/hub/devices")).body as DeviceListResponse;
    expect(listing.recovery_codes_remaining).toBe(10);
  });

  it("answers a revoke of a device that isn't there with 404", async () => {
    expect((await call("DELETE", "/api/hub/devices/nope")).status).toBe(404);
  });

  it("answers an approval of a request that isn't there with 404", async () => {
    expect((await call("POST", "/api/hub/devices/pending/nope/approve")).status).toBe(404);
  });
});
