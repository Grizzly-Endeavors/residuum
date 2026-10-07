import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InstanceInfo } from "./generated/InstanceInfo";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
import { RemoteAccessStore } from "./remote-access.svelte";
import { toast } from "./toast.svelte";

function status(instances: InstanceInfo[]): RemoteAccessStatus {
  return {
    state: "ready",
    detail: null,
    user: "bear",
    slug: "laptop",
    hosts: null,
    certificate: null,
    pins: [],
    recovery_code_pending: false,
    recovery_code: null,
    instances,
    siblings: [],
    join: null,
    pending_joins: [],
  };
}

const laptop: InstanceInfo = {
  slug: "laptop",
  display_name: "Laptop",
  active: true,
  connected: true,
};
const desktop: InstanceInfo = {
  slug: "desktop",
  display_name: "Desktop",
  active: false,
  connected: true,
};

let calls: string[];
let current: RemoteAccessStatus;
let activateAnswer: () => Response;
const reload = vi.fn();

beforeEach(() => {
  vi.useFakeTimers();
  calls = [];
  current = status([laptop, desktop]);
  activateAnswer = () => new Response(null, { status: 204 });
  reload.mockReset();
  vi.stubGlobal("window", { location: { reload } });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${input}`);
      if (input === "/api/hub/remote-access/status") {
        return Promise.resolve(new Response(JSON.stringify(current)));
      }
      return Promise.resolve(activateAnswer());
    }),
  );
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

describe("RemoteAccessStore", () => {
  it("keeps only instances with a valid slug", async () => {
    current = status([
      laptop,
      { ...desktop, slug: "a/../b" },
      { ...desktop, slug: 'x"onclick=1' },
      { ...desktop, slug: "Upper" },
    ]);
    const store = new RemoteAccessStore();
    await store.refresh();
    expect(store.instances.map((instance) => instance.slug)).toEqual(["laptop"]);
  });

  it("reads now and again every half minute until stopped", async () => {
    const store = new RemoteAccessStore();
    const stop = store.follow();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(calls).toHaveLength(2);
    stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(2);
  });

  it("keeps the last status when a read fails", async () => {
    const store = new RemoteAccessStore();
    await store.refresh();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new TypeError("offline"))),
    );
    await store.refresh();
    expect(store.instances).toHaveLength(2);
  });

  it("activates an instance by its encoded slug, then reloads after a moment", async () => {
    const store = new RemoteAccessStore();
    await store.refresh();
    await store.activate("desktop");
    expect(calls).toContain("POST /api/hub/remote-access/instances/desktop/activate");
    expect(store.switching).toBe("desktop");
    expect(reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1499);
    expect(reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(reload).toHaveBeenCalledOnce();
  });

  it("does nothing for the active instance, an unknown one or a slug that isn't valid", async () => {
    const store = new RemoteAccessStore();
    current = status([laptop, desktop, { ...desktop, slug: "a/../b" }]);
    await store.refresh();
    calls.length = 0;
    await store.activate("laptop");
    await store.activate("nowhere");
    await store.activate("a/../b");
    expect(calls).toEqual([]);
    expect(store.switching).toBeNull();
  });

  it("says so and stays put when the switch fails", async () => {
    activateAnswer = () => new Response(JSON.stringify({ error: "No." }), { status: 409 });
    const store = new RemoteAccessStore();
    await store.refresh();
    await store.activate("desktop");
    await vi.advanceTimersByTimeAsync(5000);
    expect(reload).not.toHaveBeenCalled();
    expect(store.switching).toBeNull();
    expect([...toast.toasts.values()].some((entry) => entry.kind === "error")).toBe(true);
  });
});
