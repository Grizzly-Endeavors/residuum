// What the worker shows for a push and where a click on it leads, as pure
// functions. The hub sends the payload in
// `docs/systems-usage/notifications.md`; the worker shows a notification for
// every push, readable or not, because browsers punish a push that shows
// nothing: Safari can revoke the subscription and Chrome shows a generic one.

import type { PushPayload } from "../lib/generated/PushPayload";
import type { PutPushDeviceRequest } from "../lib/generated/PutPushDeviceRequest";
import { subscriptionBody } from "../lib/push-devices";

const FALLBACK_TARGET = "/home";

/** The options the worker shows a notification with. `renotify` isn't in TypeScript's lib yet. */
export interface ShownOptions extends NotificationOptions {
  renotify?: boolean;
  data: { target: string };
}

export interface ShownPush {
  title: string;
  options: ShownOptions;
  badge: number | null;
}

export function appTarget(value: unknown): string {
  if (typeof value !== "string" || !value.startsWith("/")) return FALLBACK_TARGET;
  return /^\/[/\\]/.test(value) ? FALLBACK_TARGET : value;
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

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
      ...(tag === "" ? {} : { tag, renotify: true }),
    },
    badge: typeof badge === "number" && Number.isInteger(badge) && badge >= 0 ? badge : null,
  };
}

export function readPushData(data: { json: () => unknown } | null): unknown {
  if (data === null) return null;
  try {
    return data.json();
  } catch {
    return null;
  }
}

export function clickTarget(data: unknown): string {
  const target =
    typeof data === "object" && data !== null ? (data as { target?: unknown }).target : null;
  return appTarget(target);
}

export function rotationRequest(
  subscription: PushSubscriptionJSON,
  previousEndpoint: string | undefined,
): PutPushDeviceRequest | null {
  const body = subscriptionBody(subscription);
  return body === null ? null : { subscription: body, previous_endpoint: previousEndpoint };
}

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
