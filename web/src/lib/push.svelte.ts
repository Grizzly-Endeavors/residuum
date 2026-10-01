// This browser's push notifications, and the hub's list of every device that
// gets them. Every action acts at once through the hub's
// push routes. The hub never shows a subscription's address, so which device
// is this one is kept in local storage, with the address it registered.

import {
  ApiError,
  fetchPushDevices,
  fetchPushKey,
  registerPushDevice,
  removePushDevice,
  sendPushTest,
  updatePushDevice,
} from "./api";
import { userErrorMessage, userErrorReason } from "./errors";
import type { PushDevice, PushPreferences, PushTestResult } from "./hub-types";
import { installContext, type InstallContext } from "./install";
import { base64urlBytes, defaultDeviceLabel, madeWithKey, subscriptionBody } from "./push-devices";

/** Whether this device can get notifications at all, and if not, why. */
export type PushAvailability =
  | "available"
  /** Plain HTTP: browsers allow push only on HTTPS or localhost. */
  | "insecure"
  /** An iPhone or iPad outside the installed app, where Safari offers no push. */
  | "install-first"
  /** A browser with no Web Push. */
  | "unsupported"
  /** No service worker answered: a build without one (the dev server), or one that failed to start. */
  | "no-worker";

/** The parts of a browser subscription the store uses. */
export type SubscriptionLike = Pick<PushSubscription, "endpoint" | "toJSON" | "unsubscribe"> & {
  options: Pick<PushSubscriptionOptions, "applicationServerKey">;
};

/** The parts of a registration's `pushManager` the store uses. */
export interface PushManagerLike {
  getSubscription: () => Promise<SubscriptionLike | null>;
  subscribe: (options: PushSubscriptionOptionsInit) => Promise<SubscriptionLike>;
}

/** Which hub device this browser is, and the subscription address it was registered with. */
export interface StoredDevice {
  id: string;
  endpoint: string;
}

/** What the store needs from the browser, so tests can stand in for it. */
export interface PushBrowser {
  /** Availability that is known without asking the worker. */
  availability: PushAvailability;
  /** The name a new device is offered, such as "Chrome on Android". */
  defaultLabel: string;
  permission: () => NotificationPermission;
  requestPermission: () => Promise<NotificationPermission>;
  /** The service worker registration's push manager, or null when no worker answers. */
  pushManager: () => Promise<PushManagerLike | null>;
  readStored: () => StoredDevice | null;
  writeStored: (device: StoredDevice | null) => void;
}

/** Why turning notifications on or off failed, in plain words, or null when it didn't. */
export type PushProblem = string | null;

const BLOCKED =
  "Notifications are blocked for this site. Allow them in your browser's site settings, then turn them on here.";

/** Plain words for a browser that refused to subscribe, with the raw error in the console. */
function subscribeFailure(err: DOMException): string {
  userErrorReason(err, { action: "Couldn't subscribe to push notifications." });
  return err.name === "NotAllowedError"
    ? BLOCKED
    : "Couldn't turn on notifications: this browser couldn't reach its push service. Try again in a moment, or use another browser.";
}

export class PushStore {
  availability = $state<PushAvailability>("unsupported");
  defaultLabel = $state("");
  permission = $state<NotificationPermission>("default");
  /** This browser's device id at the hub, or null while it isn't registered. */
  deviceId = $state<string | null>(null);
  /** Every registered device, oldest first; null until loaded. */
  devices = $state.raw<PushDevice[] | null>(null);
  loadError = $state("");
  /** This browser's device left the hub's list: removed elsewhere, or pruned by its push service. */
  removedElsewhere = $state(false);
  /** Turning on or off is under way. */
  busy = $state(false);

  /** This browser's device, once the list has it. */
  thisDevice = $derived(this.devices?.find((d) => d.id === this.deviceId) ?? null);
  /** The device presence is reported for: this one, while the browser lets it show notifications. */
  presenceDevice = $derived(this.permission === "granted" ? this.deviceId : null);

  #browser: PushBrowser | null = null;
  #endpoint: string | null = null;

  /** Read what this browser holds, without asking the hub or the worker. Starts over when called again. */
  start(browser: PushBrowser): void {
    this.#browser = browser;
    this.availability = browser.availability;
    this.defaultLabel = browser.defaultLabel;
    this.devices = null;
    this.loadError = "";
    this.removedElsewhere = false;
    const stored = browser.availability === "available" ? browser.readStored() : null;
    this.permission = browser.availability === "available" ? browser.permission() : "default";
    this.deviceId = stored?.id ?? null;
    this.#endpoint = stored?.endpoint ?? null;
  }

  /** Read the device list, and check this browser's subscription against what it was registered with. */
  async load(): Promise<void> {
    const browser = this.#browser;
    if (browser === null) return;
    if (this.availability === "available") this.permission = browser.permission();
    const list = this.#loadList();
    if (this.availability === "available") await this.#checkSubscription();
    await list;
  }

  async #loadList(): Promise<void> {
    try {
      const devices = await fetchPushDevices();
      this.loadError = "";
      this.devices = devices;
      if (this.deviceId !== null && !devices.some((d) => d.id === this.deviceId)) {
        this.removedElsewhere = true;
        this.#forget();
      }
    } catch (err) {
      this.loadError = userErrorMessage(err, {
        action: "Couldn't load the devices that get notifications.",
      });
    }
  }

  /** A subscription the browser replaced or dropped leaves this device unregistered. */
  async #checkSubscription(): Promise<void> {
    const manager = await this.#manager();
    if (manager === null) return;
    const current = await manager.getSubscription().catch(() => null);
    if (this.deviceId !== null && current?.endpoint !== this.#endpoint) this.#forget();
  }

  async #manager(): Promise<PushManagerLike | null> {
    const manager = (await this.#browser?.pushManager()) ?? null;
    if (manager === null) this.availability = "no-worker";
    return manager;
  }

  #forget(): void {
    this.deviceId = null;
    this.#endpoint = null;
    this.#browser?.writeStored(null);
  }

  #replace(device: PushDevice): void {
    const others = (this.devices ?? []).filter((d) => d.id !== device.id);
    const at = this.devices?.findIndex((d) => d.id === device.id) ?? -1;
    this.devices = at < 0 ? [...others, device] : others.toSpliced(at, 0, device);
  }

  /**
   * Turn notifications on here under `label`. Call it straight from the click:
   * Safari asks for permission only in answer to one.
   */
  async enable(label: string): Promise<PushProblem> {
    const browser = this.#browser;
    if (browser === null || this.busy) return null;
    this.busy = true;
    try {
      this.permission = await browser.requestPermission();
      if (this.permission === "denied") return BLOCKED;
      if (this.permission !== "granted") {
        return "Notifications stay off until you allow them when your browser asks.";
      }
      const manager = await this.#manager();
      if (manager === null) return null;
      const key = base64urlBytes(await fetchPushKey());
      let held = await manager.getSubscription();
      // A subscription made with another key (an earlier install's) can't get this hub's pushes.
      if (held !== null && !madeWithKey(held.options.applicationServerKey, key)) {
        await held.unsubscribe();
        held = null;
      }
      const subscription =
        held ?? (await manager.subscribe({ userVisibleOnly: true, applicationServerKey: key }));
      const body = subscriptionBody(subscription.toJSON());
      if (body === null) throw new DOMException("the subscription has no keys", "AbortError");
      const device = await registerPushDevice(body, label);
      this.deviceId = device.id;
      this.#endpoint = body.endpoint;
      browser.writeStored({ id: device.id, endpoint: body.endpoint });
      this.removedElsewhere = false;
      this.#replace(device);
      return null;
    } catch (err) {
      return err instanceof DOMException
        ? subscribeFailure(err)
        : userErrorMessage(err, { action: "Couldn't turn on notifications." });
    } finally {
      this.busy = false;
    }
  }

  /** Turn notifications off here: the hub forgets the device, and the browser drops its subscription. */
  async disable(): Promise<PushProblem> {
    const id = this.deviceId;
    if (id === null || this.busy) return null;
    this.busy = true;
    try {
      await this.remove(id);
    } catch (err) {
      return userErrorMessage(err, { action: "Couldn't turn off notifications." });
    } finally {
      this.busy = false;
    }
    try {
      const subscription = await (await this.#browser?.pushManager())?.getSubscription();
      await subscription?.unsubscribe();
    } catch (err) {
      // The hub already sends nothing here; the browser's subscription just lingers.
      userErrorReason(err, { action: "Couldn't drop the browser's push subscription." });
    }
    return null;
  }

  /** Stop sending to a device. Throws `ApiError`; one already gone counts as removed. */
  async remove(id: string): Promise<void> {
    try {
      await removePushDevice(id);
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 404)) throw err;
    }
    this.devices = (this.devices ?? []).filter((d) => d.id !== id);
    if (id === this.deviceId) this.#forget();
  }

  /** Rename this device. Throws `ApiError`. */
  async rename(label: string): Promise<void> {
    if (this.deviceId === null) return;
    this.#replace(await updatePushDevice(this.deviceId, { label }));
  }

  /** Turn one kind of notification on or off for this device. Throws `ApiError`. */
  async setPreference(name: keyof PushPreferences, on: boolean): Promise<void> {
    if (this.deviceId === null) return;
    this.#replace(await updatePushDevice(this.deviceId, { preferences: { [name]: on } }));
  }

  /** Send the test notification here, then read the list again for its result. Throws `ApiError`. */
  async sendTest(): Promise<PushTestResult> {
    if (this.deviceId === null) return { delivered: false, error: null };
    try {
      return await sendPushTest(this.deviceId);
    } finally {
      // A push service that no longer knows the device makes the hub remove it.
      await this.#loadList();
    }
  }
}

export const push = new PushStore();

const STORAGE_KEY = "residuum-push-device";

/** How long to wait for the service worker before saying it isn't running. */
const WORKER_WAIT_MS = 10_000;

function readStored(): StoredDevice | null {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (typeof parsed !== "object" || parsed === null) return null;
    const { id, endpoint } = parsed as Partial<StoredDevice>;
    return typeof id === "string" && typeof endpoint === "string" ? { id, endpoint } : null;
  } catch {
    return null;
  }
}

function writeStored(device: StoredDevice | null): void {
  try {
    if (device === null) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, JSON.stringify(device));
  } catch {
    // Storage is off: this page still works, and the next one starts unregistered.
  }
}

export function pushAvailability(context: InstallContext, win: Window = window): PushAvailability {
  if (!context.secure) return "insecure";
  // Every browser on iOS offers push only to the app added to the Home Screen.
  if (context.ios && !context.installed) return "install-first";
  const supported =
    "serviceWorker" in win.navigator && "PushManager" in win && "Notification" in win;
  return supported ? "available" : "unsupported";
}

/**
 * The registration's push manager. Builds wait for the worker the page
 * registers; the dev server registers none, so `ready` would never settle
 * there, and only a registration that already exists counts.
 */
async function registrationPushManager(): Promise<PushManagerLike | null> {
  const container = navigator.serviceWorker;
  const registration = __SERVICE_WORKER__
    ? await Promise.race([
        container.ready,
        new Promise<undefined>((resolve) => setTimeout(resolve, WORKER_WAIT_MS)),
      ])
    : await container.getRegistration();
  return registration?.pushManager ?? null;
}

/** The real browser, for `push.start`. */
export function browserPush(): PushBrowser {
  const context = installContext();
  return {
    availability: pushAvailability(context),
    defaultLabel: defaultDeviceLabel(navigator.userAgent, context.installed),
    permission: () => Notification.permission,
    requestPermission: () => Notification.requestPermission(),
    pushManager: registrationPushManager,
    readStored,
    writeStored,
  };
}
