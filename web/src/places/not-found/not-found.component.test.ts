import { describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { HOME } from "../../lib/routes";
import { fireEvent, render, screen } from "../../test/component";
import NotFound from "./NotFound.svelte";

describe("Not Found", () => {
  it("names the address that leads nowhere, as it was typed", () => {
    render(NotFound, { path: "/agent/atlas/old%20notes" });

    expect(screen.getByRole("heading", { level: 1, name: "Page not found" })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 2, name: "There's nothing at this address" }),
    ).toBeInTheDocument();
    expect(screen.getByText("/agent/atlas/old notes")).toBeInTheDocument();
  });

  it("shows an address it can't decode as it arrived", () => {
    render(NotFound, { path: "/%E0%A4%A" });
    expect(screen.getByText("/%E0%A4%A")).toBeInTheDocument();
  });

  it("offers the way back to Home and the Inbox", async () => {
    const open = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(NotFound, { path: "/nowhere" });

    await fireEvent.click(screen.getByRole("button", { name: "Go to Home" }));
    expect(open).toHaveBeenLastCalledWith(HOME);

    await fireEvent.click(screen.getByRole("button", { name: "Open Inbox" }));
    expect(open).toHaveBeenLastCalledWith({
      kind: "inbox",
      agent: null,
      tab: "active",
      item: null,
    });
  });
});
