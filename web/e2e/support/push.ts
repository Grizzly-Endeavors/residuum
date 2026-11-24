import type { BrowserContext, Page, Worker } from "@playwright/test";
import { MOCK_VAPID_PUBLIC_KEY } from "../../mock/push";
import { expect } from "./fixtures";

/** The subscription's address, which the hub never shows. */
export const FAKE_ENDPOINT = "https://push.example.test/send/e2e-device";

export async function fakePushService(
  context: BrowserContext,
  options: { registration?: boolean } = {},
): Promise<void> {
  await context.grantPermissions(["notifications"]);
  await context.addInitScript(
    ({ endpoint, p256dh, registration }) => {
      const STORAGE_KEY = "e2e-push-subscription";
      const bytes = (text: string): ArrayBuffer => {
        const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
        return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0)).buffer;
      };
      const text = (key: BufferSource | string | null | undefined): string => {
        if (typeof key === "string" || key === null || key === undefined) return key ?? "";
        const view = ArrayBuffer.isView(key) ? key : new Uint8Array(key);
        const array = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
        return btoa(String.fromCharCode(...array))
          .replace(/\+/g, "-")
          .replace(/\//g, "_")
          .replace(/=+$/, "");
      };
      const subscription = (key: string): PushSubscription =>
        ({
          endpoint,
          expirationTime: null,
          options: { applicationServerKey: bytes(key), userVisibleOnly: true },
          getKey: () => null,
          toJSON: () => ({
            endpoint,
            expirationTime: null,
            keys: { p256dh, auth: "AAAAAAAAAAAAAAAAAAAAAA" },
          }),
          unsubscribe: () => {
            localStorage.removeItem(STORAGE_KEY);
            return Promise.resolve(true);
          },
        }) as unknown as PushSubscription;

      PushManager.prototype.getSubscription = function getSubscription() {
        const key = localStorage.getItem(STORAGE_KEY);
        return Promise.resolve(key === null ? null : subscription(key));
      };
      PushManager.prototype.subscribe = function subscribe(opts) {
        const key = text(opts?.applicationServerKey);
        localStorage.setItem(STORAGE_KEY, key);
        return Promise.resolve(subscription(key));
      };
      if (registration) {
        // Chromium's headless shell reports notifications denied whatever was granted,
        // and nothing is shown here, so the page is told they're allowed.
        Object.defineProperty(Notification, "permission", { get: () => "granted" });
        Notification.requestPermission = () => Promise.resolve("granted");
        const pushManager = Object.create(PushManager.prototype) as PushManager;
        const container = navigator.serviceWorker;
        const own = container.getRegistration.bind(container);
        container.getRegistration = async (url) =>
          (await own(url)) ?? ({ pushManager } as ServiceWorkerRegistration);
      }
    },
    {
      endpoint: FAKE_ENDPOINT,
      p256dh: MOCK_VAPID_PUBLIC_KEY,
      registration: options.registration === true,
    },
  );
}

const SHOWN_TIMEOUT_MS = 10_000;

async function serviceWorkerOf(context: BrowserContext): Promise<Worker> {
  return context.serviceWorkers()[0] ?? (await context.waitForEvent("serviceworker"));
}

function watchNextShown(worker: Worker): Promise<void> {
  return worker.evaluate(() => {
    const scope = self as unknown as {
      registration: ServiceWorkerRegistration;
      pushShown: Promise<void>;
      pushSettled: boolean;
    };
    const registration = scope.registration;
    const original = registration.showNotification.bind(registration);
    const restore = (): boolean => Reflect.deleteProperty(registration, "showNotification");
    const shown = new Promise<void>((resolve, reject) => {
      registration.showNotification = (...args) => {
        restore();
        const result = original(...args);
        result.then(resolve, reject);
        return result;
      };
    });
    const settled = (): void => {
      scope.pushSettled = true;
    };
    scope.pushSettled = false;
    scope.pushShown = shown;
    void shown.then(settled, settled);
  });
}

/**
 * Deliver `payload` to the page's service worker as its push service would,
 * encrypted payload already opened, and return once the worker has shown the
 * notification for it.
 *
 * The wait is what keeps a read of the notifications from racing the
 * display. Chromium's `getNotifications()` drops every stored notification
 * the display doesn't list yet, and a notification is stored a moment before
 * it is listed. A read in between deletes it for good while the worker's
 * `showNotification` still resolves. `deliverPushMessage` returns before the
 * worker has so much as received the push, so a spec that polls straight away
 * lands in that gap now and then.
 */
export async function deliverPush(page: Page, payload: unknown): Promise<void> {
  const context = page.context();
  const worker = await serviceWorkerOf(context);
  await watchNextShown(worker);
  const cdp = await context.newCDPSession(page);
  const registered = new Promise<string>((resolve) => {
    cdp.on("ServiceWorker.workerRegistrationUpdated", ({ registrations }) => {
      const found = registrations.find((r) => !r.isDeleted);
      if (found !== undefined) resolve(found.registrationId);
    });
  });
  await cdp.send("ServiceWorker.enable");
  await cdp.send("ServiceWorker.deliverPushMessage", {
    origin: new URL(page.url()).origin,
    registrationId: await registered,
    data: JSON.stringify(payload),
  });
  await cdp.detach();
  await expect
    .poll(() => worker.evaluate(() => (self as unknown as { pushSettled: boolean }).pushSettled), {
      message: "the worker showed no notification for the push",
      timeout: SHOWN_TIMEOUT_MS,
    })
    .toBe(true);
  await worker.evaluate(() => (self as unknown as { pushShown: Promise<void> }).pushShown);
}

export function shownNotifications(
  page: Page,
): Promise<{ title: string; body: string; tag: string; target: unknown }[]> {
  return page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    const shown = await registration.getNotifications();
    return shown.map((n) => ({
      title: n.title,
      body: n.body,
      tag: n.tag,
      target: (n.data as { target?: unknown } | null)?.target,
    }));
  });
}
