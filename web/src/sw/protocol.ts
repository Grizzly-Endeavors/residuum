// What the page and the service worker say to each other. Both sides import
// this file, so it uses neither the window's nor the worker's globals.

/** The worker's script. It sits at the root, so its scope is the whole app. */
export const WORKER_URL = "/sw.js";

/** Tells a waiting worker to take over from the active one. */
export const SKIP_WAITING_MESSAGE = { type: "skip-waiting" } as const;

/** A message the page posts to the worker. */
export type PageMessage = typeof SKIP_WAITING_MESSAGE;

export function isPageMessage(data: unknown): data is PageMessage {
  return (
    typeof data === "object" &&
    data !== null &&
    "type" in data &&
    data.type === SKIP_WAITING_MESSAGE.type
  );
}

/**
 * A message the worker posts to a window of the app: a notification was
 * clicked, and the window it brought forward should show `target`, an app
 * path such as `/inbox?item=atlas:note-1`. The page routes there itself, so
 * nothing reloads and unsaved work is asked about first.
 */
export interface OpenTargetMessage {
  type: "open-target";
  target: string;
}

/**
 * A message the worker posts to every window of the app: `pushsubscriptionchange`
 * fired, and the worker re-registered this browser's device under `endpoint`.
 * Each page's own record of the device (kept in local storage) must follow,
 * or the next check there would wrongly think the device was dropped.
 */
export interface EndpointChangedMessage {
  type: "endpoint-changed";
  endpoint: string;
}

export type WorkerMessage = OpenTargetMessage | EndpointChangedMessage;

export function openTargetMessage(target: string): OpenTargetMessage {
  return { type: "open-target", target };
}

export function endpointChangedMessage(endpoint: string): EndpointChangedMessage {
  return { type: "endpoint-changed", endpoint };
}

export function isWorkerMessage(data: unknown): data is WorkerMessage {
  if (typeof data !== "object" || data === null || !("type" in data)) return false;
  if (data.type === "open-target") return "target" in data && typeof data.target === "string";
  if (data.type === "endpoint-changed") {
    return "endpoint" in data && typeof data.endpoint === "string";
  }
  return false;
}
