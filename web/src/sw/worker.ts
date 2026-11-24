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
declare const __SW_PRECACHE__: readonly string[];
declare const __SW_VERSION__: string;

const CACHE = shellCacheName(__SW_VERSION__);
const GENERATIONS_CACHE = "residuum-generations";
const GENERATIONS_KEY = "/generations";

/**
 * Files fetched at once while installing. Residuum Cloud's relay lets 50
 * requests through a tunnel at a time and answers the rest 503, and the page
 * is loading its own files meanwhile.
 */
const PRECACHE_CONCURRENCY = 6;

const MATCH = { ignoreVary: true } as const;

const precached: ReadonlySet<string> = new Set(__SW_PRECACHE__);

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

async function fetchFile(url: string): Promise<Response> {
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
  await self.clients.claim();
}

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

async function navigate(request: Request): Promise<Response> {
  let failure: Response | null = null;
  try {
    const response = await fetch(request);
    if (!isGatewayFailure(response.status)) return response;
    failure = response;
  } catch {}
  const shell = await (await caches.open(CACHE)).match(SHELL_URL, MATCH);
  return shell ?? failure ?? Response.error();
}

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
  } catch {}
}

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

self.addEventListener("push", (event) => {
  event.waitUntil(showPush(event.data));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(openTarget(clickTarget(event.notification.data)));
});

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
  } catch {}
}

async function tellWindows(message: EndpointChangedMessage): Promise<void> {
  const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  for (const window of windows) window.postMessage(message);
}

self.addEventListener("pushsubscriptionchange", (event) => {
  event.waitUntil(handleSubscriptionChange(event.oldSubscription?.endpoint));
});
