// What the worker shows for a push and where a click on it leads, as pure
// functions. The hub sends the payload in
// `docs/systems-usage/notifications.md`; the worker shows a notification for
// every push, readable or not, because browsers punish a push that shows
// nothing: Safari can revoke the subscription and Chrome shows a generic one.

import type { PushPayload } from "../lib/generated/PushPayload";
import type { PutPushDeviceRequest } from "../lib/generated/PutPushDeviceRequest";
import { subscriptionBody } from "../lib/push-devices";

/** Where a click goes when the push named nowhere the app can open. */
const FALLBACK_TARGET = "/home";

/** The options the worker shows a notification with. `renotify` isn't in TypeScript's lib yet. */
export interface ShownOptions extends NotificationOptions {
  renotify?: boolean;
  /** Kept on the notification for the click. */
  data: { target: string };
}

/** A notification to show, and the app badge to set (null leaves the badge as it is). */
export interface ShownPush {
  title: string;
  options: ShownOptions;
  badge: number | null;
}

/**
 * A path in this app, or the fallback. A payload is the hub's, but the click
 * must never open another site: `//host` and `/\host` are other origins to a
 * browser.
 */
export function appTarget(value: unknown): string {
  if (typeof value !== "string" || !value.startsWith("/")) return FALLBACK_TARGET;
  return /^\/[/\\]/.test(value) ? FALLBACK_TARGET : value;
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/** The notification for a push's decrypted JSON, whatever arrived. */
export function notificationFor(payload: unknown): ShownPush {
  const fields: Partial<Record<keyof PushPayload, unknown>> =
    typeof payload === "object" && payload !== null ? payload : {};
  const title = text(fields.title);
  const body = text(fields.body);
  const tag = text(fields.tag);
  const badge = fields.badge;
  return {
    title: title === "" ? "Residuum" : title,
    options: {
      body: title === "" && body === "" ? "Something needs your attention. Open Residuum." : body,
      icon: "/icons/icon-192.png",
      data: { target: appTarget(fields.target) },
      // A later push with the same tag replaces the earlier one, and still alerts.
      ...(tag === "" ? {} : { tag, renotify: true }),
    },
    badge: typeof badge === "number" && Number.isInteger(badge) && badge >= 0 ? badge : null,
  };
}

/** The decrypted JSON of a push, or null when it has none or it isn't JSON. */
export function readPushData(data: { json: () => unknown } | null): unknown {
  if (data === null) return null;
  try {
    return data.json();
  } catch {
    return null;
  }
}

/** Where a click on a notification goes, from the data the worker showed it with. */
export function clickTarget(data: unknown): string {
  const target =
    typeof data === "object" && data !== null ? (data as { target?: unknown }).target : null;
  return appTarget(target);
}

/**
 * The hub's request for a browser-rotated subscription: the new subscription
 * under the endpoint it replaces, so the hub updates the device in place
 * instead of registering a second one. Null when the new subscription has no
 * keys, which never happens in practice but `PushSubscriptionJSON` allows it.
 */
export function rotationRequest(
  subscription: PushSubscriptionJSON,
  previousEndpoint: string | undefined,
): PutPushDeviceRequest | null {
  const body = subscriptionBody(subscription);
  return body === null ? null : { subscription: body, previous_endpoint: previousEndpoint };
}

/** The window a click brings forward: the focused one, else a visible one, else any; null when none is open. */
export function windowForClick<W extends { focused: boolean; visibilityState: string }>(
  windows: readonly W[],
): W | null {
  return (
    windows.find((w) => w.focused) ??
    windows.find((w) => w.visibilityState === "visible") ??
    windows[0] ??
    null
  );
}
