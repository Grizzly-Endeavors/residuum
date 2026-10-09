import { act, cleanup, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import NotificationsHarness from "../../test/ui/NotificationsHarness.svelte";
import { composerClearance } from "../composer-clearance.svelte";
import { notifications } from "../notifications.svelte";
import { router } from "../router.svelte";
import { toast } from "../toast.svelte";

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

afterEach(async () => {
  vi.useRealTimers();
  cleanup();
  await vi.waitFor(() => {
    expect((window.history.state as { overlay?: string } | null)?.overlay).toBeUndefined();
  });
});

function errorRegion(): HTMLElement {
  return screen.getByRole("alert");
}

function politeRegion(): HTMLElement {
  return screen.getByRole("status");
}

describe("ToastRegion", () => {
  it("announces errors assertively and everything else politely, from the overlay host", async () => {
    render(NotificationsHarness);
    toast.error("Couldn't save the settings.");
    toast.info("Reloaded settings for every agent.");
    toast.success("Saved.");
    await act();
    expect(errorRegion()).toHaveTextContent("Couldn't save the settings.");
    expect(errorRegion()).not.toHaveTextContent("Saved.");
    expect(politeRegion()).toHaveTextContent("Reloaded settings for every agent.");
    expect(politeRegion()).toHaveTextContent("Saved.");
    expect(errorRegion()).toHaveAttribute("aria-atomic", "false");
    expect(politeRegion().closest("[data-overlay-host]")).not.toBeNull();
  });

  describe("clearing the composer", () => {
    function region(): HTMLElement {
      const found = document.querySelector<HTMLElement>(".ui-toasts");
      if (found === null) throw new Error("no toast region");
      return found;
    }

    it("rides above the composer's top edge, wherever it is, and drops back without one", async () => {
      vi.stubGlobal(
        "ResizeObserver",
        class {
          observe(): void {}
          disconnect(): void {}
        },
      );
      Object.defineProperty(document.documentElement, "clientHeight", {
        configurable: true,
        get: () => 900,
      });
      render(NotificationsHarness);
      await act();
      // With no composer to reach, a phone keeps clear of a sheet's footer above the bottom bar.
      expect(region()).toHaveAttribute("data-clearance", "away");

      const composer = document.createElement("form");
      document.body.append(composer);
      composer.getBoundingClientRect = () => ({ top: 700 }) as DOMRect;
      composer.getClientRects = () => [{}] as unknown as DOMRectList;
      const release = composerClearance.track(composer);
      await act();
      expect(region()).toHaveAttribute("data-clearance", "lifted");
      expect(region().style.getPropertyValue("--toast-lift")).toBe("200px");

      release();
      composer.remove();
      await act();
      expect(region()).toHaveAttribute("data-clearance", "away");
    });
  });

  it("keeps the store's timings: 4 seconds, 10 with an action, errors until dismissed", async () => {
    vi.useFakeTimers();
    render(NotificationsHarness);
    toast.info("Reloaded settings.");
    toast.success("Removed the key.", { label: "Undo", onClick: () => {} });
    toast.error("Couldn't reach Residuum.");
    await act();
    await act(() => vi.advanceTimersByTime(4001));
    expect(politeRegion()).not.toHaveTextContent("Reloaded settings.");
    expect(politeRegion()).toHaveTextContent("Removed the key.");
    await act(() => vi.advanceTimersByTime(6000));
    expect(politeRegion()).not.toHaveTextContent("Removed the key.");
    await act(() => vi.advanceTimersByTime(60_000));
    expect(errorRegion()).toHaveTextContent("Couldn't reach Residuum.");
  });

  it("runs a toast's action once and dismisses it, and dismisses from its close button", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(NotificationsHarness);
    toast.success("Removed the key.", { label: "Undo", onClick });
    toast.error("Couldn't reach Residuum.");
    await act();
    await user.click(within(politeRegion()).getByRole("button", { name: "Undo" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(politeRegion().textContent.trim()).toBe("");
    await user.click(within(errorRegion()).getByRole("button", { name: "Dismiss notification" }));
    expect(errorRegion().textContent.trim()).toBe("");
  });

  it("stays reachable above an open dialog: never inert, never a layer", async () => {
    const user = userEvent.setup();
    render(NotificationsHarness);
    await user.click(screen.getByRole("button", { name: "Open dialog" }));
    toast.success("Removed the key.", { label: "Undo", onClick: () => {} });
    await act();
    const region = politeRegion();
    expect(region.closest("[inert]")).toBeNull();
    await user.click(within(region).getByRole("button", { name: "Undo" }));
    // The press didn't count as outside the dialog.
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
  });
});

describe("RecentNotifications", () => {
  function surfaceSamples(): void {
    notifications.surface("notice", "scout finished its weekly review.");
    notifications.surface(
      "error",
      "atlas couldn't reach Anthropic.",
      "provider anthropic answered 401 Unauthorized",
    );
  }

  async function openRecent(): Promise<HTMLElement> {
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Recent notifications" }));
    return screen.getByRole("dialog", { name: "Recent notifications" });
  }

  it("lists the history newest first, with its kind, its details and how long ago", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(new Date("2026-03-14T12:00:00Z"));
    render(NotificationsHarness);
    surfaceSamples();
    const dialog = await openRecent();
    const items = within(dialog).getAllByRole("listitem");
    expect(items.map((item) => item.querySelector("p")?.textContent.trim())).toEqual([
      "Error: atlas couldn't reach Anthropic.",
      "Notice: scout finished its weekly review.",
    ]);
    const [error] = items;
    if (error === undefined) throw new Error("no items");
    expect(within(error).getByText("just now").tagName).toBe("TIME");
    const details = within(error).getByRole("button", { name: "Details" });
    expect(details).toHaveAttribute("aria-expanded", "false");
    await userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) }).click(details);
    expect(within(error).getByText("provider anthropic answered 401 Unauthorized")).toBeVisible();

    await act(() => vi.advanceTimersByTime(5 * 60_000));
    expect(within(error).getByText("5m ago")).toBeInTheDocument();
  });

  it("says when there is nothing yet, and offers no Clear", async () => {
    render(NotificationsHarness);
    const dialog = await openRecent();
    expect(dialog).toHaveTextContent("Nothing yet.");
    expect(within(dialog).queryByRole("button", { name: "Clear all" })).toBeNull();
  });

  it("clears, then undoes from the dialog, where focus stays on the button", async () => {
    const user = userEvent.setup();
    render(NotificationsHarness);
    surfaceSamples();
    const dialog = await openRecent();
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    await user.click(within(dialog).getByRole("button", { name: "Clear all" }));
    expect(notifications.history).toEqual([]);
    expect(dialog).toHaveTextContent("Cleared 2 notifications.");
    expect(within(politeRegion()).getByText("Cleared 2 notifications.")).toBeInTheDocument();
    const undo = within(dialog).getByRole("button", { name: "Undo" });
    expect(undo).toHaveFocus();
    await user.click(undo);
    expect(notifications.history.map((item) => item.message)).toEqual([
      "atlas couldn't reach Anthropic.",
      "scout finished its weekly review.",
    ]);
    expect(within(dialog).getByRole("button", { name: "Clear all" })).toHaveFocus();
    // Undone here, the toast's Undo goes with it.
    expect(politeRegion().textContent.trim()).toBe("");
  });

  it("undoes once from the toast above the dialog, keeping what arrived since ahead", async () => {
    const user = userEvent.setup();
    render(NotificationsHarness);
    surfaceSamples();
    const dialog = await openRecent();
    await user.click(within(dialog).getByRole("button", { name: "Clear all" }));
    notifications.surface("system", "Gateway is reloading…");
    await act();
    const undo = within(politeRegion())
      .getByText("Cleared 2 notifications.")
      .closest(".ui-toast")
      ?.querySelector("button");
    if (!undo) throw new Error("no Undo on the toast");
    await user.click(undo);
    expect(notifications.history.map((item) => item.message)).toEqual([
      "Gateway is reloading…",
      "atlas couldn't reach Anthropic.",
      "scout finished its weekly review.",
    ]);
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(3);
    expect(within(dialog).queryByRole("button", { name: "Undo" })).toBeNull();
  });
});
