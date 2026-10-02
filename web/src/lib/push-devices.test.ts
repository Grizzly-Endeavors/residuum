import { describe, expect, it } from "vitest";
import type { PushDevice } from "./hub-types";
import {
  base64urlBytes,
  defaultDeviceLabel,
  deliveryOf,
  madeWithKey,
  subscriptionBody,
} from "./push-devices";

function device(fields: Partial<PushDevice>): PushDevice {
  return {
    id: "d1",
    label: "Phone",
    created_at: "2026-03-01T12:00:00Z",
    last_success_at: null,
    last_failure: null,
    preferences: {
      inbox_item: true,
      agent_failed: true,
      outbound_unreachable: false,
      reply_while_away: false,
    },
    ...fields,
  };
}

const failure = { at: "2026-03-14T11:00:00Z", status: 413, message: "The message was too big." };

describe("deliveryOf", () => {
  it("is nothing yet for a device nothing was sent to", () => {
    expect(deliveryOf(device({}))).toEqual({ kind: "none" });
  });

  it("is failing while the last failure is newer than the last success", () => {
    expect(deliveryOf(device({ last_failure: failure }))).toEqual({
      kind: "failing",
      at: failure.at,
      message: failure.message,
    });
    expect(
      deliveryOf(device({ last_failure: failure, last_success_at: "2026-03-14T10:00:00Z" })),
    ).toMatchObject({ kind: "failing" });
  });

  it("is delivered once a success follows the failure the hub still keeps", () => {
    expect(
      deliveryOf(device({ last_failure: failure, last_success_at: "2026-03-14T11:30:00Z" })),
    ).toEqual({ kind: "delivered", at: "2026-03-14T11:30:00Z" });
  });
});

describe("defaultDeviceLabel", () => {
  it.each([
    [
      "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36",
      "Chrome on Android",
    ],
    [
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
      "Safari on iPhone",
    ],
    [
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36 Edg/129.0",
      "Edge on Windows",
    ],
    ["Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0", "Firefox on Linux"],
    ["SomethingElse/1.0", "Browser"],
  ])("names %s as %s", (agent, label) => {
    expect(defaultDeviceLabel(agent, false)).toBe(label);
  });

  it("names the installed app as the app", () => {
    expect(
      defaultDeviceLabel(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/604.1",
        true,
      ),
    ).toBe("Residuum app on iPhone");
  });
});

describe("subscriptionBody", () => {
  it("takes the address and both keys", () => {
    expect(
      subscriptionBody({
        endpoint: "https://push.example/1",
        expirationTime: null,
        keys: { p256dh: "pk", auth: "secret" },
      }),
    ).toEqual({ endpoint: "https://push.example/1", keys: { p256dh: "pk", auth: "secret" } });
  });

  it("is null for a subscription missing a part", () => {
    expect(subscriptionBody({ endpoint: "https://push.example/1", keys: {} })).toBeNull();
    expect(subscriptionBody({ keys: { p256dh: "pk", auth: "secret" } })).toBeNull();
  });
});

describe("the application server key", () => {
  it("decodes base64url with or without padding", () => {
    expect([...base64urlBytes("_-8")]).toEqual([0xff, 0xef]);
    expect([...base64urlBytes("AQID")]).toEqual([1, 2, 3]);
  });

  it("matches a subscription made with the same key, and no other", () => {
    const key = base64urlBytes("AQID");
    expect(madeWithKey(new Uint8Array([1, 2, 3]).buffer, key)).toBe(true);
    expect(madeWithKey(new Uint8Array([1, 2, 4]).buffer, key)).toBe(false);
    expect(madeWithKey(new Uint8Array([1, 2]).buffer, key)).toBe(false);
    expect(madeWithKey(null, key)).toBe(false);
  });
});
