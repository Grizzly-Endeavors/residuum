// The app's service worker, served at `/sw.js`. It keeps the app
// shell so the app opens with no network, and nothing else: it never caches or
// answers `/api`, sockets, webhooks or the cloud callback (see `rules.ts`). It
// also shows the hub's push notifications and opens the app where one leads
// (see `push.ts`).
//
// `build/service-worker.ts` bundles this file and fills in the two constants
// below at build time: the files to precache, and a version derived from them.
// Every build whose files differ produces a different worker, which is how a
// browser notices an update.

import type { PushKeyResponse } from "../lib/generated/PushKeyResponse";
import { base64urlBytes } from "../lib/push-devices";
import {
  type EndpointChangedMessage,
  endpointChangedMessage,
  isPageMessage,
  openTargetMessage,
} from "./protocol";
import {
  clickTarget,
  notificationFor,
  readPushData,
  rotationRequest,
  windowForClick,
} from "./push";
import {
  handlingOf,
  isGatewayFailure,
  isHashedAsset,
  parseGenerations,
  recordActivation,
  SHELL_URL,
  shellCacheName,
  staleCaches,
  type Generations,
} from "./rules";

declare const self: ServiceWorkerGlobalScope;
/** The files of this build the shell needs: the document, everything under `/assets/`, the icons. */
declare const __SW_PRECACHE__: readonly string[];
/** Changes whenever the build's files do. */
declare const __SW_VERSION__: string;

const CACHE = shellCacheName(__SW_VERSION__);
/** Which shell versions have been active, kept apart from the version caches so housekeeping can't delete it. */
const GENERATIONS_CACHE = "residuum-generations";
const GENERATIONS_KEY = "/generations";

/**
 * Files fetched at once while installing. Residuum Cloud's relay lets 50
 * requests through a tunnel at a time and answers the rest 503, and the page
 * is loading its own files meanwhile.
 */
const PRECACHE_CONCURRENCY = 6;

/** One copy of each file per cache, so the response's `Vary` is no reason to miss. */
const MATCH = { ignoreVary: true } as const;

const precached: ReadonlySet<string> = new Set(__SW_PRECACHE__);

/** Put every file of this build in this version's cache. */
async function precache(): Promise<void> {
  const cache = await caches.open(CACHE);
  const queue = [...__SW_PRECACHE__];
  const fetchNext = async (): Promise<void> => {
    for (let url = queue.shift(); url !== undefined; url = queue.shift()) {
      await cache.put(url, await fetchFile(url));
    }
  };
  await Promise.all(Array.from({ length: PRECACHE_CONCURRENCY }, fetchNext));
}

/**
 * A file for the cache, or an error that fails the install: a worker that
 * can't hold the whole shell is discarded and the active one keeps serving.
 * A redirect is an error too, since what it leads to (the relay's sign-in
 * page, say) is not the file.
 */
async function fetchFile(url: string): Promise<Response> {
  // A file that keeps its name must come from the server, not the HTTP cache,
  // or an older copy could land in the new version. A hashed file can't be
  // older than its name, and the page has often fetched it already.
  const response = await fetch(url, { cache: isHashedAsset(url) ? "default" : "reload" });
  if (!response.ok || response.redirected) {
    const outcome = response.redirected ? "redirected" : `answered ${String(response.status)}`;
    throw new Error(`couldn't precache ${url}: the server ${outcome}`);
  }
  return response;
}

async function activate(): Promise<void> {
  const generations = await recordGenerations();
  const stale = staleCaches(await caches.keys(), generations);
  await Promise.all(stale.map((name) => caches.delete(name)));
  // The page that registered this worker has none yet, and takes it on now.
  await self.clients.claim();
}

/** Note this version as the active one and return what to keep: it and the one it replaced. */
async function recordGenerations(): Promise<Generations> {
  const store = await caches.open(GENERATIONS_CACHE);
  const stored = await store.match(GENERATIONS_KEY);
  const recorded = stored === undefined ? null : parseGenerations(await readJson(stored));
  const generations = recordActivation(recorded, __SW_VERSION__);
  await store.put(GENERATIONS_KEY, Response.json(generations));
  return generations;
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    return null;
  }
}

/**
 * A page load: the network first, so a rebuilt app arrives with its new
 * document. When the hub can't be reached the cached shell stands in, and the
 * app's own hub banner says why nothing loads.
 */
async function navigate(request: Request): Promise<Response> {
  let failure: Response | null = null;
  try {
    const response = await fetch(request);
    if (!isGatewayFailure(response.status)) return response;
    failure = response;
  } catch {
    // Offline, or the hub refused the connection.
  }
  const shell = await (await caches.open(CACHE)).match(SHELL_URL, MATCH);
  return shell ?? failure ?? Response.error();
}

/**
 * A file of the app. This version's cache answers first. Under a page opened
 * before an update the hashed files it was built with are no longer in this
 * version, and the previous version's cache still holds them.
 */
async function fromCache(request: Request): Promise<Response> {
  const path = new URL(request.url).pathname;
  const own = await (await caches.open(CACHE)).match(path, MATCH);
  return own ?? (await caches.match(path, MATCH)) ?? fetch(request);
}

self.addEventListener("install", (event) => {
  event.waitUntil(precache());
});

self.addEventListener("activate", (event) => {
  event.waitUntil(activate());
});

// A new worker waits for the page to say so: taking over mid-session would swap
// the files under an open page. The page asks once the person chooses to reload.
self.addEventListener("message", (event) => {
  if (isPageMessage(event.data)) event.waitUntil(self.skipWaiting());
});

self.addEventListener("fetch", (event) => {
  switch (handlingOf(event.request, self.location.origin, precached)) {
    case "navigate":
      event.respondWith(navigate(event.request));
      break;
    case "cache-first":
      event.respondWith(fromCache(event.request));
      break;
    case "pass":
      break;
  }
});

/** Show the push's notification and set the app badge to the unread count it carries. */
async function showPush(data: PushMessageData | null): Promise<void> {
  const shown = notificationFor(readPushData(data));
  await Promise.all([
    self.registration.showNotification(shown.title, shown.options),
    shown.badge === null ? null : showBadge(shown.badge),
  ]);
}

/** The badge is a courtesy: a browser without it, or one that refuses, changes nothing else. */
async function showBadge(count: number): Promise<void> {
  const nav = self.navigator as Partial<Pick<WorkerNavigator, "setAppBadge" | "clearAppBadge">>;
  try {
    await (count > 0 ? nav.setAppBadge?.(count) : nav.clearAppBadge?.());
  } catch {
    // The app isn't installed, or badges are off for it.
  }
}

/** Bring a window of the app forward on `target`, or open one there. */
async function openTarget(target: string): Promise<void> {
  const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  const chosen = windowForClick(windows);
  if (chosen === null) {
    await self.clients.openWindow(target);
    return;
  }
  try {
    await chosen.focus();
  } catch {
    // A browser may refuse focus; the window still goes to the target.
  }
  chosen.postMessage(openTargetMessage(target));
}

// Every push shows a notification: suppressing one while the app is in use is
// the hub's job, since a push that shows nothing is punished.
self.addEventListener("push", (event) => {
  event.waitUntil(showPush(event.data));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(openTarget(clickTarget(event.notification.data)));
});

/**
 * Re-subscribe under the hub's key after the browser rotates this device's
 * subscription on its own (a key expiring, its storage being cleared), and
 * send the new subscription back naming the endpoint it replaces, so the hub
 * updates the same device instead of registering a second one. A browser
 * that doesn't support this event still falls back to the ordinary path: the
 * next delivery against the stale endpoint gets 404 or 410, and the hub
 * prunes the device and raises its own notice.
 */
async function handleSubscriptionChange(previousEndpoint: string | undefined): Promise<void> {
  try {
    const keyResponse = await fetch("/api/hub/push/key");
    if (!keyResponse.ok) throw new Error(`the hub answered ${String(keyResponse.status)}`);
    const { public_key: publicKey } = (await keyResponse.json()) as PushKeyResponse;
    const subscription = await self.registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: base64urlBytes(publicKey),
    });
    const request = rotationRequest(subscription.toJSON(), previousEndpoint);
    if (request === null) throw new Error("the new subscription has no keys");
    const putResponse = await fetch("/api/hub/push/devices", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!putResponse.ok) throw new Error(`the hub answered ${String(putResponse.status)}`);
    await tellWindows(endpointChangedMessage(request.subscription.endpoint));
  } catch {
    // Resubscribing or telling the hub failed. Nothing more to do here: once
    // the stale endpoint next fails delivery, the hub's own prune notice
    // covers it.
  }
}

/** Tell every open window of the app `message`. */
async function tellWindows(message: EndpointChangedMessage): Promise<void> {
  const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  for (const window of windows) window.postMessage(message);
}

self.addEventListener("pushsubscriptionchange", (event) => {
  event.waitUntil(handleSubscriptionChange(event.oldSubscription?.endpoint));
});
