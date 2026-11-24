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

export type PushAvailability =
  | "available"
  /** Plain HTTP: browsers allow push only on HTTPS or localhost. */
  | "insecure"
  /** An iPhone or iPad outside the installed app, where Safari offers no push. */
  | "install-first"
  | "unsupported"
  | "no-worker";

export type SubscriptionLike = Pick<PushSubscription, "endpoint" | "toJSON" | "unsubscribe"> & {
  options: Pick<PushSubscriptionOptions, "applicationServerKey">;
};

export interface PushManagerLike {
  getSubscription: () => Promise<SubscriptionLike | null>;
  subscribe: (options: PushSubscriptionOptionsInit) => Promise<SubscriptionLike>;
}

export interface StoredDevice {
  id: string;
  endpoint: string;
}

export interface PushBrowser {
  availability: PushAvailability;
  defaultLabel: string;
  permission: () => NotificationPermission;
  requestPermission: () => Promise<NotificationPermission>;
  pushManager: () => Promise<PushManagerLike | null>;
  readStored: () => StoredDevice | null;
  writeStored: (device: StoredDevice | null) => void;
}

export type PushProblem = string | null;

const BLOCKED =
  "Notifications are blocked for this site. Allow them in your browser's site settings, then turn them on here.";

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
  deviceId = $state<string | null>(null);
  devices = $state.raw<PushDevice[] | null>(null);
  loadError = $state("");
  removedElsewhere = $state(false);
  busy = $state(false);

  thisDevice = $derived(this.devices?.find((d) => d.id === this.deviceId) ?? null);
  presenceDevice = $derived(this.permission === "granted" ? this.deviceId : null);

  #browser: PushBrowser | null = null;
  #endpoint: string | null = null;

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

  deviceRotated(endpoint: string): void {
    if (this.deviceId === null) return;
    this.#endpoint = endpoint;
    this.#browser?.writeStored({ id: this.deviceId, endpoint });
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
      userErrorReason(err, { action: "Couldn't drop the browser's push subscription." });
    }
    return null;
  }

  async remove(id: string): Promise<void> {
    try {
      await removePushDevice(id);
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 404)) throw err;
    }
    this.devices = (this.devices ?? []).filter((d) => d.id !== id);
    if (id === this.deviceId) this.#forget();
  }

  async rename(label: string): Promise<void> {
    if (this.deviceId === null) return;
    this.#replace(await updatePushDevice(this.deviceId, { label }));
  }

  async setPreference(name: keyof PushPreferences, on: boolean): Promise<void> {
    if (this.deviceId === null) return;
    this.#replace(await updatePushDevice(this.deviceId, { preferences: { [name]: on } }));
  }

  async sendTest(): Promise<PushTestResult> {
    if (this.deviceId === null) return { delivered: false, error: null };
    try {
      return await sendPushTest(this.deviceId);
    } finally {
      await this.#loadList();
    }
  }
}

export const push = new PushStore();

const STORAGE_KEY = "residuum-push-device";

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
  } catch {}
}

export function pushAvailability(context: InstallContext, win: Window = window): PushAvailability {
  if (!context.secure) return "insecure";
  // Every browser on iOS offers push only to the app added to the Home Screen.
  if (context.ios && !context.installed) return "install-first";
  const supported =
    "serviceWorker" in win.navigator && "PushManager" in win && "Notification" in win;
  return supported ? "available" : "unsupported";
}

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
