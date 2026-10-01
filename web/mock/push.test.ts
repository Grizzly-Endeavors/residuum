import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { PushDevice } from "../src/lib/generated/PushDevice";
import { MOCK_VAPID_PUBLIC_KEY } from "./push";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

/** A well-formed 16-byte authentication secret. */
const AUTH = "AAAAAAAAAAAAAAAAAAAAAA";

/** What a browser's `PushSubscription.toJSON()` gives for `endpoint`. */
function subscription(endpoint: string): object {
  return {
    endpoint,
    expirationTime: null,
    keys: { p256dh: MOCK_VAPID_PUBLIC_KEY, auth: AUTH },
  };
}

describe("the Web Push routes", () => {
  let harness: MockServerHarness;

  const call = (
    method: string,
    path: string,
    body?: unknown,
  ): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

  const devices = async (): Promise<PushDevice[]> => {
    const res = await call("GET", "/api/hub/push/devices");
    expect(res.status).toBe(200);
    return (res.body as { devices: PushDevice[] }).devices;
  };

  const register = async (endpoint: string, label?: string): Promise<PushDevice> => {
    const res = await call("PUT", "/api/hub/push/devices", {
      subscription: subscription(endpoint),
      label,
    });
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return (res.body as { device: PushDevice }).device;
  };

  beforeEach(async () => {
    harness = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await harness.close();
  });

  it("serves a public key a browser can subscribe with", async () => {
    const res = await call("GET", "/api/hub/push/key");
    expect(res).toEqual({ status: 200, body: { public_key: MOCK_VAPID_PUBLIC_KEY } });
    // 65 bytes of base64url without padding.
    expect(Buffer.from(MOCK_VAPID_PUBLIC_KEY, "base64url")).toHaveLength(65);
  });

  it("starts with no devices", async () => {
    expect(await devices()).toEqual([]);
  });

  it("registers a device with the default preferences and never returns its endpoint", async () => {
    const device = await register("https://push.example.com/send/abc", "Laptop");

    expect(device).toEqual({
      id: "push-device-1",
      label: "Laptop",
      created_at: "2026-03-14T12:00:00.000Z",
      last_success_at: null,
      last_failure: null,
      preferences: {
        inbox_item: true,
        agent_failed: true,
        outbound_unreachable: false,
        reply_while_away: false,
      },
    });
    expect(await devices()).toEqual([device]);
    expect(JSON.stringify(await devices())).not.toContain("push.example.com");
  });

  it("names a device that registers without a label", async () => {
    expect((await register("https://push.example.com/send/abc")).label).toBe("Unnamed device");
  });

  it("updates the device registered for the same endpoint instead of adding another", async () => {
    const first = await register("https://push.example.com/send/abc", "Laptop");

    const res = await call("PUT", "/api/hub/push/devices", {
      subscription: subscription("https://push.example.com/send/abc"),
      label: "  Work laptop ",
      preferences: { reply_while_away: true },
    });

    expect(res.status).toBe(200);
    const { device } = res.body as { device: PushDevice };
    expect(device.id).toBe(first.id);
    expect(device.label).toBe("Work laptop");
    expect(device.preferences).toEqual({
      inbox_item: true,
      agent_failed: true,
      outbound_unreachable: false,
      reply_while_away: true,
    });
    expect(await devices()).toHaveLength(1);

    await register("https://push.example.com/send/other", "Phone");
    expect(await devices()).toHaveLength(2);
  });

  it.each([
    [
      "an http endpoint",
      { subscription: subscription("http://push.example.com/a") },
      "the subscription's endpoint must be an https:// address",
    ],
    [
      "an endpoint that isn't an address",
      { subscription: subscription("not a url") },
      "the subscription's endpoint isn't a web address (invalid URL)",
    ],
    [
      "a public key that isn't a point",
      {
        subscription: {
          endpoint: "https://push.example.com/a",
          keys: { p256dh: "AAAA", auth: AUTH },
        },
      },
      "the subscription's p256dh key isn't a base64url P-256 public key (65 bytes starting with 0x04)",
    ],
    [
      "a short auth secret",
      {
        subscription: {
          endpoint: "https://push.example.com/a",
          keys: { p256dh: MOCK_VAPID_PUBLIC_KEY, auth: "AAAA" },
        },
      },
      "the subscription's auth secret isn't 16 base64url-encoded bytes",
    ],
    [
      "no subscription",
      { label: "x" },
      "the request body isn't valid for this route: missing field `subscription`",
    ],
    [
      "a blank label",
      { subscription: subscription("https://push.example.com/a"), label: "  " },
      "a device needs a name",
    ],
    [
      "a preference that isn't a boolean",
      {
        subscription: subscription("https://push.example.com/a"),
        preferences: { inbox_item: "yes" },
      },
      "the request body isn't valid for this route: invalid type for `inbox_item`: expected a boolean",
    ],
  ])("refuses a registration with %s", async (_what, body, error) => {
    const res = await call("PUT", "/api/hub/push/devices", body);
    expect(res).toEqual({ status: 400, body: { error } });
    expect(await devices()).toEqual([]);
  });

  it("refuses a body that isn't JSON", async () => {
    const res = await fetchJson(`${harness.baseUrl}/api/hub/push/devices`, {
      method: "PUT",
      body: "{ nope",
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain(
      "the request body isn't valid for this route",
    );
  });

  it("changes a device's label and only the preferences it names", async () => {
    const { id } = await register("https://push.example.com/send/abc", "Laptop");

    const res = await call("PATCH", `/api/hub/push/devices/${id}`, {
      label: "Desk",
      preferences: { agent_failed: false },
    });

    expect(res.status).toBe(200);
    const { device } = res.body as { device: PushDevice };
    expect(device.label).toBe("Desk");
    expect(device.preferences).toEqual({
      inbox_item: true,
      agent_failed: false,
      outbound_unreachable: false,
      reply_while_away: false,
    });
    expect(await devices()).toEqual([device]);
  });

  it("answers 404 for an unknown device and 400 for a patch that changes nothing", async () => {
    const { id } = await register("https://push.example.com/send/abc", "Laptop");

    expect(await call("PATCH", "/api/hub/push/devices/nope", { label: "x" })).toEqual({
      status: 404,
      body: { error: "there is no notification device with id 'nope'" },
    });
    expect(await call("PATCH", `/api/hub/push/devices/${id}`, {})).toEqual({
      status: 400,
      body: { error: "the request must set a label or preferences" },
    });
    expect(await call("PATCH", `/api/hub/push/devices/${id}`, { label: " " })).toEqual({
      status: 400,
      body: { error: "a device needs a name" },
    });
  });

  it("removes a device with 204 and answers 404 the second time", async () => {
    const { id } = await register("https://push.example.com/send/abc", "Laptop");

    const res = await fetch(`${harness.baseUrl}/api/hub/push/devices/${id}`, { method: "DELETE" });
    expect(res.status).toBe(204);
    expect(await res.text()).toBe("");
    expect(await devices()).toEqual([]);
    expect((await call("DELETE", `/api/hub/push/devices/${id}`)).status).toBe(404);
  });

  it("delivers a test notification and records the success", async () => {
    const { id } = await register("https://push.example.com/send/abc", "Laptop");

    const res = await call("POST", `/api/hub/push/devices/${id}/test`);

    expect(res).toEqual({ status: 200, body: { delivered: true, error: null } });
    const [device] = await devices();
    expect(device?.last_success_at).toBe("2026-03-14T12:00:00.000Z");
    expect(device?.last_failure).toBeNull();
    expect((await call("POST", "/api/hub/push/devices/nope/test")).status).toBe(404);
  });

  it("is a route of the hub, not of an agent", async () => {
    const res = await call("GET", "/api/agents/atlas/push/key");
    expect(res.status).toBe(404);
  });

  it("forgets its devices when the mock is reset", async () => {
    await register("https://push.example.com/send/abc", "Laptop");
    harness.hub.reset();

    expect(await devices()).toEqual([]);
    expect((await register("https://push.example.com/send/abc", "Laptop")).id).toBe(
      "push-device-1",
    );
  });
});
