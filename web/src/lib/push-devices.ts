import type { PushDevice, WebPushSubscription } from "./hub-types";

export type Delivery =
  | { kind: "none" }
  | { kind: "delivered"; at: string }
  | { kind: "failing"; at: string; message: string };

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

export function defaultDeviceLabel(userAgent: string, installed: boolean): string {
  const browser = installed
    ? "Residuum app"
    : (BROWSERS.find(([pattern]) => pattern.test(userAgent))?.[1] ?? "Browser");
  // iPadOS reports itself as a Mac.
  const system = SYSTEMS.find(([pattern]) => pattern.test(userAgent))?.[1];
  return system === undefined ? browser : `${browser} on ${system}`;
}

export function subscriptionBody(json: PushSubscriptionJSON): WebPushSubscription | null {
  const { endpoint, keys } = json;
  const p256dh = keys?.p256dh;
  const auth = keys?.auth;
  if (endpoint === undefined || p256dh === undefined || auth === undefined) return null;
  return { endpoint, keys: { p256dh, auth } };
}

export function base64urlBytes(text: string): Uint8Array<ArrayBuffer> {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64.padEnd(Math.ceil(base64.length / 4) * 4, "="));
  return Uint8Array.from(binary, (char) => char.charCodeAt(0));
}

export function madeWithKey(applicationServerKey: ArrayBuffer | null, key: Uint8Array): boolean {
  if (applicationServerKey === null) return false;
  const held = new Uint8Array(applicationServerKey);
  return held.length === key.length && held.every((byte, i) => byte === key[i]);
}
