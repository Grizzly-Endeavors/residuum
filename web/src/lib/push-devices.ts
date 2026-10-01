// Plain facts about push devices, with no browser or store: how delivery to a
// device is going, the name a new device is offered, and the browser's
// subscription as the hub takes it.

import type { PushDevice, WebPushSubscription } from "./hub-types";

/** How delivery to a device is going, from its last success and its last failure. */
export type Delivery =
  | { kind: "none" }
  | { kind: "delivered"; at: string }
  | { kind: "failing"; at: string; message: string };

/**
 * The hub keeps a device's last failure after a success, so delivery is
 * failing only when the failure is the newer of the two.
 */
export function deliveryOf(device: PushDevice): Delivery {
  const failure = device.last_failure;
  const success = device.last_success_at;
  if (failure !== null && (success === null || Date.parse(failure.at) > Date.parse(success))) {
    return { kind: "failing", at: failure.at, message: failure.message };
  }
  return success === null ? { kind: "none" } : { kind: "delivered", at: success };
}

const BROWSERS: readonly (readonly [RegExp, string])[] = [
  [/Edg(A|iOS)?\//, "Edge"],
  [/Firefox\/|FxiOS\//, "Firefox"],
  [/Chrome\/|CriOS\//, "Chrome"],
  [/Safari\//, "Safari"],
];

const SYSTEMS: readonly (readonly [RegExp, string])[] = [
  [/iPhone|iPod/, "iPhone"],
  [/iPad/, "iPad"],
  [/Android/, "Android"],
  [/CrOS/, "ChromeOS"],
  [/Mac OS X|Macintosh/, "Mac"],
  [/Windows/, "Windows"],
  [/Linux/, "Linux"],
];

/** A name for this device that its owner would recognize in a list: "Chrome on Android", "Residuum app on iPhone". */
export function defaultDeviceLabel(userAgent: string, installed: boolean): string {
  const browser = installed
    ? "Residuum app"
    : (BROWSERS.find(([pattern]) => pattern.test(userAgent))?.[1] ?? "Browser");
  // iPadOS reports itself as a Mac.
  const system = SYSTEMS.find(([pattern]) => pattern.test(userAgent))?.[1];
  return system === undefined ? browser : `${browser} on ${system}`;
}

/** The browser's subscription JSON as the hub's register route takes it, or null when it lacks a part. */
export function subscriptionBody(json: PushSubscriptionJSON): WebPushSubscription | null {
  const { endpoint, keys } = json;
  const p256dh = keys?.p256dh;
  const auth = keys?.auth;
  if (endpoint === undefined || p256dh === undefined || auth === undefined) return null;
  return { endpoint, keys: { p256dh, auth } };
}

/** The bytes of base64url text, for a subscription's `applicationServerKey`. */
export function base64urlBytes(text: string): Uint8Array<ArrayBuffer> {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64.padEnd(Math.ceil(base64.length / 4) * 4, "="));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

/** Whether a subscription was made with `key`; one made with another key can't receive the hub's pushes. */
export function madeWithKey(applicationServerKey: ArrayBuffer | null, key: Uint8Array): boolean {
  if (applicationServerKey === null) return false;
  const held = new Uint8Array(applicationServerKey);
  return held.length === key.length && held.every((byte, i) => byte === key[i]);
}
