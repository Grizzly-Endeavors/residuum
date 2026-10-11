import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen } from "../test/component";
import { appUpdate } from "../lib/app-update.svelte";
import { router } from "../lib/router.svelte";
import UpdateBanner from "./UpdateBanner.svelte";

beforeEach(() => {
  appUpdate.ready = false;
  appUpdate.applying = false;
  appUpdate.dismissed = false;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("UpdateBanner", () => {
  it("shows nothing until an update is ready", () => {
    render(UpdateBanner);
    expect(screen.queryByText("Update ready.")).toBeNull();
  });

  it("says an update is ready and offers Reload", () => {
    appUpdate.ready = true;
    render(UpdateBanner);
    expect(screen.getByRole("status")).toHaveTextContent(
      "Update ready. Reload to use the latest version of Residuum.",
    );
    expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
  });

  it("reloads through the router's guard, so unsaved work is asked about first", async () => {
    appUpdate.ready = true;
    const apply = vi.spyOn(appUpdate, "apply").mockResolvedValue();
    const confirmReload = vi.spyOn(router.guard, "confirmReload").mockResolvedValue(false);
    render(UpdateBanner);
    await userEvent.setup().click(screen.getByRole("button", { name: "Reload" }));

    expect(apply).toHaveBeenCalledOnce();
    // `apply` asks through the function it is given; the guard is what answers.
    const confirm = apply.mock.calls[0]?.[0];
    await expect(confirm?.()).resolves.toBe(false);
    expect(confirmReload).toHaveBeenCalledOnce();
  });

  it("shows Reload as under way once it has been confirmed", () => {
    appUpdate.ready = true;
    appUpdate.applying = true;
    render(UpdateBanner);
    expect(screen.getByRole("button", { name: "Reload" })).toHaveAttribute("aria-busy", "true");
  });

  it("goes away when dismissed, until the page loads again", async () => {
    appUpdate.ready = true;
    render(UpdateBanner);
    await userEvent.setup().click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("Update ready.")).toBeNull();
    expect(appUpdate.ready).toBe(true);
  });
});
