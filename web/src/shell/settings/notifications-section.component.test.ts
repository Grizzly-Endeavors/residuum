import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PushDevice } from "../../lib/hub-types";
import {
  push,
  type PushBrowser,
  type PushManagerLike,
  type StoredDevice,
  type SubscriptionLike,
} from "../../lib/push.svelte";
import { settingsModel, type AllScopeModel } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { fakeAgentConfig } from "../../test/fake-config";
import { installHelp } from "../app-actions.svelte";
import NotificationsSection from "./NotificationsSection.svelte";
import { waitFor } from "../../test/wait";

// Settings → Notifications: this browser's push (turning it on, its name,
// what it's told about, a test send, how delivery is going), the other
// devices, and the staged contact for push services. The browser and the
// hub's push routes are stand-ins.

/** The hub's VAPID key, base64url, and its bytes. */
const KEY = "AQID";
const ENDPOINT = "https://push.example.test/1";

interface Call {
  method: string;
  url: string;
  body: unknown;
}

function deviceOf(id: string, label: string, fields: Partial<PushDevice> = {}): PushDevice {
  return {
    id,
    label,
    created_at: "2026-03-01T12:00:00Z",
    last_success_at: null,
    last_failure: null,
    preferences: {
      inbox_item: true,
      agent_failed: true,
      outbound_unreachable: false,
      reply_while_away: false,
    },
    ...fields,
  };
}

let devices: PushDevice[];
let calls: Call[];
let failNext: { method: string; status: number } | null;
let testAnswer: { delivered: boolean; error: string | null };
let scope: AllScopeModel;
let count = 0;

/** The hub's push routes over a fake fetch, in front of the config routes. */
function serve(hub = 'timezone = "UTC"\n'): void {
  const config = fakeAgentConfig(`notifications-${String(++count)}`, { hub });
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    if (!url.startsWith("/api/hub/push/")) return config.handler(url, init);
    const body: unknown = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    calls.push({ method, url, body });
    if (failNext?.method === method) {
      const { status } = failNext;
      failNext = null;
      return new Response(JSON.stringify({ error: "the hub refused it" }), { status });
    }
    if (url === "/api/hub/push/key") return jsonResponse({ public_key: KEY });
    if (url === "/api/hub/push/devices" && method === "GET") return jsonResponse({ devices });
    if (url === "/api/hub/push/devices" && method === "PUT") {
      const { label } = body as { label: string };
      const added = deviceOf("new-1", label);
      devices = [...devices, added];
      return jsonResponse({ device: added });
    }
    const id = decodeURIComponent(/devices\/([^/]+)/.exec(url)?.[1] ?? "");
    const at = devices.findIndex((d) => d.id === id);
    if (at < 0) return new Response(JSON.stringify({ error: "no such device" }), { status: 404 });
    if (url.endsWith("/test")) return jsonResponse(testAnswer);
    if (method === "DELETE") {
      devices = devices.filter((d) => d.id !== id);
      return new Response(null, { status: 204 });
    }
    const change = body as { label?: string; preferences?: Partial<PushDevice["preferences"]> };
    const current = devices[at] as PushDevice;
    const next = {
      ...current,
      label: change.label ?? current.label,
      preferences: { ...current.preferences, ...change.preferences },
    };
    devices = devices.map((d) => (d.id === id ? next : d));
    return jsonResponse({ device: next });
  });
}

/** A browser that can subscribe, with what it holds and was asked. */
function fakeBrowser(
  fields: Partial<PushBrowser> = {},
  held: { subscribed?: boolean; stored?: StoredDevice | null } = {},
): PushBrowser & { stored: StoredDevice | null; subscribed: boolean } {
  const state = {
    stored: held.stored ?? null,
    subscribed: held.subscribed ?? false,
  };
  const subscription: SubscriptionLike = {
    endpoint: ENDPOINT,
    options: { applicationServerKey: new Uint8Array([1, 2, 3]).buffer },
    toJSON: () => ({ endpoint: ENDPOINT, keys: { p256dh: "pk", auth: "secret" } }),
    unsubscribe: () => {
      state.subscribed = false;
      return Promise.resolve(true);
    },
  };
  const manager: PushManagerLike = {
    getSubscription: () => Promise.resolve(state.subscribed ? subscription : null),
    subscribe: () => {
      state.subscribed = true;
      return Promise.resolve(subscription);
    },
  };
  return Object.assign(state, {
    availability: "available" as const,
    defaultLabel: "Chrome on Linux",
    permission: () => "default" as NotificationPermission,
    requestPermission: () => Promise.resolve("granted" as NotificationPermission),
    pushManager: () => Promise.resolve(manager),
    readStored: () => state.stored,
    writeStored: (device: StoredDevice | null) => {
      state.stored = device;
    },
    ...fields,
  });
}

/** This browser registered as "Laptop", with its subscription. */
function registered(fields: Partial<PushBrowser> = {}): ReturnType<typeof fakeBrowser> {
  return fakeBrowser(
    { permission: () => "granted", ...fields },
    { subscribed: true, stored: { id: "laptop", endpoint: ENDPOINT } },
  );
}

async function open(browser: PushBrowser): Promise<void> {
  push.start(browser);
  scope = settingsModel.all();
  await scope.reload();
  render(NotificationsSection, { scope, section: "notifications" });
  await settle();
  await settle();
}

const messages = (): string[] => [...toast.toasts.values()].map((shown) => shown.message);
const group = (name: string): HTMLElement => screen.getByRole("region", { name });

beforeEach(() => {
  devices = [
    deviceOf("laptop", "Laptop", { last_success_at: "2026-03-14T10:00:00Z" }),
    deviceOf("phone", "Pixel", {
      last_success_at: "2026-03-14T09:00:00Z",
      last_failure: {
        at: "2026-03-14T11:00:00Z",
        status: 413,
        message: "The message was too big.",
      },
    }),
  ];
  calls = [];
  failNext = null;
  testAnswer = { delivered: true, error: null };
  serve();
});

afterEach(() => {
  scope.discard();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  installHelp.open = false;
});

describe("this device, before notifications are on", () => {
  it("turns them on under the name given: permission, subscription, then the hub", async () => {
    const requested = vi.fn(() => Promise.resolve("granted" as NotificationPermission));
    const browser = fakeBrowser({ requestPermission: requested });
    await open(browser);
    expect(screen.getByText("Off", { exact: true })).toBeInTheDocument();
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();

    const name = screen.getByLabelText("Name for this device");
    expect(name).toHaveValue("Chrome on Linux");
    await fireEvent.input(name, { target: { value: "Work laptop" } });
    await fireEvent.click(screen.getByRole("button", { name: "Turn on notifications" }));
    await waitFor(() => expect(screen.getByText("On", { exact: true })).toBeInTheDocument());

    expect(requested).toHaveBeenCalledOnce();
    expect(calls.find((call) => call.method === "PUT")?.body).toEqual({
      subscription: { endpoint: ENDPOINT, keys: { p256dh: "pk", auth: "secret" } },
      label: "Work laptop",
    });
    expect(browser.stored).toEqual({ id: "new-1", endpoint: ENDPOINT });
    expect(screen.getByText("On", { exact: true })).toBeInTheDocument();
    expect(screen.getByLabelText("Device name")).toHaveValue("Work laptop");
    expect(screen.getByRole("switch", { name: /New inbox items/ })).toBeChecked();
    expect(push.presenceDevice).toBe("new-1");
  });

  it("stays off and says so when the browser isn't allowed to notify", async () => {
    await open(fakeBrowser({ requestPermission: () => Promise.resolve("denied") }));
    await fireEvent.click(screen.getByRole("button", { name: "Turn on notifications" }));
    await settle();
    expect(screen.getByRole("alert")).toHaveTextContent(/blocked for this site/);
    expect(calls.some((call) => call.method === "PUT")).toBe(false);
  });

  it("says when the browser's push service can't be reached", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const failing: PushManagerLike = {
      getSubscription: () => Promise.resolve(null),
      subscribe: () => Promise.reject(new DOMException("push service error", "AbortError")),
    };
    await open(fakeBrowser({ pushManager: () => Promise.resolve(failing) }));
    await fireEvent.click(screen.getByRole("button", { name: "Turn on notifications" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(/couldn't reach its push service/),
    );
  });

  it("offers nothing to press while notifications are blocked", async () => {
    await open(fakeBrowser({ permission: () => "denied" }));
    expect(screen.getByText("Blocked", { exact: true })).toBeInTheDocument();
    expect(screen.getByText(/blocked for this site/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn on notifications" })).not.toBeInTheDocument();
  });

  it("says when this device was removed elsewhere, and forgets it", async () => {
    const browser = registered();
    devices = devices.filter((d) => d.id !== "laptop");
    await open(browser);
    expect(screen.getByText(/turned off from another device/)).toBeInTheDocument();
    expect(browser.stored).toBeNull();
    expect(screen.getByRole("button", { name: "Turn on notifications" })).toBeInTheDocument();
  });

  it("treats a subscription the browser replaced as off", async () => {
    const browser = registered();
    browser.subscribed = false;
    await open(browser);
    expect(browser.stored).toBeNull();
    expect(screen.getByRole("button", { name: "Turn on notifications" })).toBeInTheDocument();
    // The hub still lists the old registration among the other devices, to remove.
    expect(screen.getByRole("list", { name: "Other devices" })).toHaveTextContent("Laptop");
  });
});

describe("why this device can't get notifications", () => {
  it.each([
    ["insecure", /only over a secure connection/],
    ["unsupported", /can't show notifications from Residuum/],
    ["no-worker", /background worker isn't running/],
  ] as const)("says so when it is %s", async (availability, note) => {
    await open(fakeBrowser({ availability }));
    expect(screen.getByText(note)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn on notifications" })).not.toBeInTheDocument();
    // The other devices are still listed and can be removed from here.
    expect(screen.getByRole("list", { name: "Other devices" })).toHaveTextContent("Pixel");
  });

  it("finds out there is no worker when the registration doesn't answer", async () => {
    await open(fakeBrowser({ pushManager: () => Promise.resolve(null) }));
    expect(screen.getByText(/background worker isn't running/)).toBeInTheDocument();
  });

  it("points an iPhone to the Home Screen app, with the steps", async () => {
    await open(fakeBrowser({ availability: "install-first" }));
    expect(screen.getByText(/only in the app on your Home Screen/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Show me how" }));
    expect(installHelp.open).toBe(true);
  });
});

describe("this device, with notifications on", () => {
  it("shows its name, its last delivery and what it's told about", async () => {
    await open(registered());
    expect(screen.getByText("On", { exact: true })).toBeInTheDocument();
    expect(screen.getByLabelText("Device name")).toHaveValue("Laptop");
    expect(group("This device")).toHaveTextContent(/Last notification sent/);
    expect(screen.getByRole("switch", { name: /New inbox items/ })).toBeChecked();
    expect(screen.getByRole("switch", { name: /Replies while you're away/ })).not.toBeChecked();
  });

  it("changes one kind of notification at once, and puts it back when the hub refuses", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await open(registered());
    await fireEvent.click(screen.getByRole("switch", { name: /Replies while you're away/ }));
    await waitFor(() => {
      expect(calls.at(-1)?.method).toBe("PATCH");
    });
    await waitFor(() => {
      expect(push.thisDevice?.preferences.reply_while_away).toBe(true);
    });
    expect(calls.at(-1)).toMatchObject({
      method: "PATCH",
      url: "/api/hub/push/devices/laptop",
      body: { preferences: { reply_while_away: true } },
    });
    expect(screen.getByRole("switch", { name: /Replies while you're away/ })).toBeChecked();

    failNext = { method: "PATCH", status: 500 };
    await fireEvent.click(screen.getByRole("switch", { name: /New inbox items/ }));
    await waitFor(() => {
      expect(messages().join(" ")).toMatch(/Couldn't change what this device is told about/);
    });
    expect(screen.getByRole("switch", { name: /New inbox items/ })).toBeChecked();
  });

  it("renames it, and won't send a blank name", async () => {
    await open(registered());
    const name = screen.getByLabelText("Device name");
    await fireEvent.input(name, { target: { value: "  " } });
    expect(screen.getByText("A device needs a name.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();

    await fireEvent.input(name, { target: { value: "Desk" } });
    await fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await settle();
    expect(calls.at(-1)).toMatchObject({ method: "PATCH", body: { label: "Desk" } });
    expect(messages()).toContain("Renamed this device to Desk.");
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();
  });

  it("sends a test and says whether the push service took it", async () => {
    await open(registered());
    await fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    await waitFor(() => {
      expect(messages()).toContain("Sent a test notification. It should appear here soon.");
    });

    testAnswer = { delivered: false, error: "The push service rejected the signing key." };
    await fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    await waitFor(() => {
      expect(messages()).toContain(
        "The test notification wasn't delivered. The push service rejected the signing key.",
      );
    });
  });

  it("turns off: the hub forgets it and the browser drops its subscription", async () => {
    const browser = registered();
    await open(browser);
    await fireEvent.click(screen.getByRole("button", { name: "Turn off on this device" }));
    await waitFor(() => {
      expect(browser.subscribed).toBe(false);
    });
    expect(calls.at(-1)).toMatchObject({ method: "DELETE", url: "/api/hub/push/devices/laptop" });
    expect(browser.stored).toBeNull();
    expect(screen.getByText("Off", { exact: true })).toBeInTheDocument();
    expect(push.presenceDevice).toBeNull();
  });
});

describe("other devices", () => {
  it("lists them with a failure that came after their last success, and removes one", async () => {
    await open(registered());
    const list = screen.getByRole("list", { name: "Other devices" });
    expect(list).not.toHaveTextContent("Laptop");
    expect(list).toHaveTextContent("Pixel");
    expect(list).toHaveTextContent(/Last notification failed .*The message was too big\./);

    await fireEvent.click(screen.getByRole("button", { name: "Remove Pixel" }));
    await settle();
    expect(calls.at(-1)).toMatchObject({ method: "DELETE", url: "/api/hub/push/devices/phone" });
    expect(messages()).toContain("Pixel no longer gets notifications.");
    expect(screen.getByText("No other devices get notifications.")).toBeInTheDocument();
  });

  it("says when the list can't be read, with Try again", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    failNext = { method: "GET", status: 500 };
    await open(fakeBrowser({ availability: "insecure" }));
    expect(
      screen.getByText(/Couldn't load the devices that get notifications/),
    ).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await settle();
    expect(screen.getByRole("list", { name: "Other devices" })).toHaveTextContent("Pixel");
  });
});

describe("the contact for push services", () => {
  it("is staged in the hub's [push] table, and flags one the hub can't use", async () => {
    await open(fakeBrowser({ availability: "insecure" }));
    const contact = screen.getByLabelText("Contact");
    await fireEvent.input(contact, { target: { value: "me@example.com" } });
    expect(screen.getByText(/only a mailto: or https:\/\/ contact/)).toBeInTheDocument();

    await fireEvent.input(contact, { target: { value: "mailto:me@example.com" } });
    expect(screen.queryByText(/only a mailto: or https:\/\/ contact/)).not.toBeInTheDocument();
    expect(scope.dirty).toBe(true);
    expect(scope.config.push_contact).toBe("mailto:me@example.com");
  });
});
