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
