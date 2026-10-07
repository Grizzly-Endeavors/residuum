import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InstanceInfo } from "../lib/generated/InstanceInfo";
import { RemoteAccessStore } from "../lib/remote-access.svelte";
import { toast } from "../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import InstanceSwitcher from "./InstanceSwitcher.svelte";

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
  connected: false,
};

let instances: InstanceInfo[];
let calls: string[];
const reload = vi.fn();

function statusWith(list: InstanceInfo[]): unknown {
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
    instances: list,
    siblings: [],
    join: null,
    pending_joins: [],
  };
}

async function open(): Promise<ReturnType<typeof userEvent.setup>> {
  const store = new RemoteAccessStore();
  await store.refresh();
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  render(InstanceSwitcher, { store });
  await settle();
  return user;
}

beforeEach(() => {
  vi.useFakeTimers();
  instances = [laptop, desktop];
  calls = [];
  reload.mockReset();
  vi.stubGlobal("location", { reload });
  mockFetch((url, init) => {
    calls.push(`${init?.method ?? "GET"} ${url}`);
    if (url === "/api/hub/remote-access/status") return jsonResponse(statusWith(instances));
    return new Response(null, { status: 204 });
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

describe("InstanceSwitcher", () => {
  it.each([[[]], [[laptop]]])("is hidden with %j", async (list) => {
    instances = list;
    await open();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("is hidden when only one of two instances has a valid slug", async () => {
    instances = [laptop, { ...desktop, slug: "a/../b" }];
    await open();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("names the active instance and marks each row", async () => {
    const user = await open();
    const trigger = screen.getByRole("button", { name: /Instance: Laptop/ });
    expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    await user.click(trigger);
    expect(screen.getByRole("menuitem", { name: "Laptop Active" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Desktop Offline" })).toBeInTheDocument();
  });

  it("works from the keyboard", async () => {
    const user = await open();
    screen.getByRole("button", { name: /Instance:/ }).focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Laptop Active" })).toHaveFocus();
    await user.keyboard("{ArrowDown}{Enter}");
    await settle();
    expect(calls).toContain("POST /api/hub/remote-access/instances/desktop/activate");
  });

  it("switches by calling activate, says so, and reloads after a moment", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: /Instance:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Desktop Offline" }));
    await settle();
    expect(calls).toContain("POST /api/hub/remote-access/instances/desktop/activate");
    expect(screen.getByRole("status")).toHaveTextContent("Switching…");
    expect(screen.getByRole("button", { name: /Switching to Desktop…/ })).toBeDisabled();
    expect(reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1500);
    expect(reload).toHaveBeenCalledOnce();
  });

  it("does nothing when the active instance is chosen", async () => {
    const user = await open();
    await user.click(screen.getByRole("button", { name: /Instance:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Laptop Active" }));
    await settle();
    expect(calls.filter((call) => call.startsWith("POST"))).toEqual([]);
  });

  it("draws a hostile display name as text", async () => {
    instances = [
      { ...laptop, display_name: "<img src=x onerror=alert(1)>" },
      { ...desktop, display_name: '"><script>alert(1)</script>' },
    ];
    const user = await open();
    await user.click(screen.getByRole("button", { name: /Instance:/ }));
    expect(screen.getAllByText("<img src=x onerror=alert(1)>").length).toBeGreaterThan(0);
    expect(screen.getByText('"><script>alert(1)</script>')).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
  });

  it("gives an entry with a hostile slug no row and no request", async () => {
    instances = [
      laptop,
      desktop,
      { ...desktop, slug: "a/../b", display_name: "Traversal" },
      { ...desktop, slug: 'x"onclick=1', display_name: "Injected" },
    ];
    const user = await open();
    await user.click(screen.getByRole("button", { name: /Instance:/ }));
    expect(screen.queryByText("Traversal")).not.toBeInTheDocument();
    expect(screen.queryByText("Injected")).not.toBeInTheDocument();
    expect(screen.getAllByRole("menuitem")).toHaveLength(2);
  });
});
