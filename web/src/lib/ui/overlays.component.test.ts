import { act, cleanup, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import OverlayHarness from "../../test/ui/OverlayHarness.svelte";
import { router } from "../router.svelte";
import ConfirmDialog from "./ConfirmDialog.svelte";
import ConfirmHost from "./ConfirmHost.svelte";
import { confirmations, confirmLeave } from "./confirm.svelte";

/** Let the history traversals an overlay started land. */
async function settleHistory(): Promise<void> {
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
}

function scrim(): HTMLElement {
  const scrims = document.querySelectorAll<HTMLElement>("[data-overlay-scrim]");
  const top = scrims[scrims.length - 1];
  if (top === undefined) throw new Error("no overlay is open");
  return top;
}

/** A one-finger touch at `y` (and `x`), as the browser reports it. */
function touch(type: string, x: number, y: number): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  const points = type === "touchend" ? [] : [{ clientX: x, clientY: y }];
  Object.defineProperty(event, "touches", { value: points });
  return event;
}

function drag(element: HTMLElement, from: [number, number], to: [number, number]): void {
  element.dispatchEvent(touch("touchstart", ...from));
  for (let step = 1; step <= 4; step += 1) {
    const x = from[0] + ((to[0] - from[0]) * step) / 4;
    const y = from[1] + ((to[1] - from[1]) * step) / 4;
    element.dispatchEvent(touch("touchmove", x, y));
  }
  element.dispatchEvent(touch("touchend", ...to));
}

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

// Unmounting closes what a test left open, and that goes back in the history:
// let it land before the next test opens anything.
afterEach(async () => {
  cleanup();
  await settleHistory();
});

describe("Dialog", () => {
  it("opens in the overlay host as a labelled modal, focusing its first control", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    const dialog = screen.getByRole("dialog", { name: "Outer" });
    expect(dialog.tagName).toBe("DIALOG");
    expect(dialog).toHaveAttribute("open");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAccessibleDescription("The first layer.");
    expect(dialog.closest("[data-overlay-host]")).not.toBeNull();
    // The close button comes first, but a dialog starts on its content.
    expect(screen.getByRole("button", { name: "First control" })).toHaveFocus();
  });

  it("traps Tab inside, coming round from either end", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    screen.getByRole("button", { name: "Last control" }).focus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole("button", { name: "Last control" })).toHaveFocus();
  });

  it("closes on Esc and puts focus back on what opened it", async () => {
    const user = userEvent.setup();
    const onclose = vi.fn();
    render(OverlayHarness, { onclose });
    const trigger = screen.getByRole("button", { name: "Open" });
    await user.click(trigger);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onclose).toHaveBeenCalledTimes(1);
    expect(trigger).toHaveFocus();
  });

  it("keeps Esc from reaching the page behind once it has closed a layer", async () => {
    const user = userEvent.setup();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.keyboard("{Escape}");
    await user.keyboard("{Escape}");
    window.removeEventListener("keydown", behind);
    expect(behind).toHaveBeenCalledTimes(1);
  });

  it("closes from a press and release on the scrim, not from a drag that ends there", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.pointer([
      { keys: "[MouseLeft>]", target: screen.getByRole("button", { name: "First control" }) },
      { target: scrim() },
      { keys: "[/MouseLeft]", target: scrim() },
    ]);
    expect(screen.getByRole("dialog", { name: "Outer" })).toBeInTheDocument();
    await user.click(scrim());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("makes the page inert and stops it scrolling while open, and undoes both on close", async () => {
    const user = userEvent.setup();
    const { container } = render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    expect(container).toHaveAttribute("inert");
    expect(document.documentElement.style.overflow).toBe("hidden");
    await user.keyboard("{Escape}");
    expect(container).not.toHaveAttribute("inert");
    expect(document.documentElement.style.overflow).toBe("");
  });

  it("leaves inertness it didn't set alone", async () => {
    const user = userEvent.setup();
    const aside = document.createElement("aside");
    aside.setAttribute("inert", "");
    document.body.append(aside);
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.keyboard("{Escape}");
    expect(aside).toHaveAttribute("inert");
    aside.remove();
  });

  it("stacks: the inner layer is on top, the outer inert, and the topmost closes first", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    const opener = screen.getByRole("button", { name: "Open inner" });
    await user.click(opener);
    const outer = screen.getByRole("dialog", { name: "Outer" });
    const inner = screen.getByRole("dialog", { name: "Inner" });
    expect(outer.compareDocumentPosition(inner) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(outer.closest(".ui-modal-layer")).toHaveAttribute("inert");
    expect(screen.getByRole("button", { name: "Inner control" })).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Inner" })).toBeNull();
    expect(outer.closest(".ui-modal-layer")).not.toHaveAttribute("inert");
    expect(opener).toHaveFocus();

    await user.click(opener);
    await user.click(scrim());
    expect(screen.queryByRole("dialog", { name: "Inner" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Outer" })).toBeInTheDocument();
  });

  it("holds a history entry: Back closes it, and closing from the UI pops the entry", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    const start = (window.history.state as { idx: number }).idx;
    await user.click(screen.getByRole("button", { name: "Open" }));
    const entry = window.history.state as { idx: number; overlay?: string };
    expect(entry.idx).toBe(start + 1);
    expect(entry.overlay).toBeDefined();
    window.history.back();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(window.history.state).toMatchObject({ idx: start });

    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.keyboard("{Escape}");
    await settleHistory();
    expect(window.history.state).toEqual({ idx: start });
  });

  it("closes nested layers one at a time on Back", async () => {
    const user = userEvent.setup();
    render(OverlayHarness);
    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.click(screen.getByRole("button", { name: "Open inner" }));
    window.history.back();
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Inner" })).toBeNull();
    });
    expect(screen.getByRole("dialog", { name: "Outer" })).toBeInTheDocument();
    window.history.back();
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Outer" })).toBeNull();
    });
  });
});

describe("Sheet and Drawer", () => {
  function sized(dialog: HTMLElement, size: number): HTMLElement {
    Object.defineProperty(dialog, "offsetHeight", { value: size, configurable: true });
    Object.defineProperty(dialog, "offsetWidth", { value: size, configurable: true });
    return dialog;
  }

  it("springs a sheet back after a short drag, and closes it after a long one", async () => {
    const user = userEvent.setup();
    const onclose = vi.fn();
    render(OverlayHarness, { kind: "sheet", onclose });
    await user.click(screen.getByRole("button", { name: "Open" }));
    const sheet = sized(screen.getByRole("dialog", { name: "Switch agent" }), 400);
    // Slow and short: under a third of its height, so it stays.
    vi.spyOn(Event.prototype, "timeStamp", "get").mockReturnValueOnce(0).mockReturnValue(1000);
    drag(sheet, [100, 500], [100, 560]);
    await act();
    expect(sheet).toBeInTheDocument();
    expect(sheet.style.transform).toBe("");
    vi.restoreAllMocks();
    drag(sheet, [100, 500], [100, 700]);
    await act();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onclose).toHaveBeenCalledTimes(1);
  });

  it("follows the finger while dragging, unless motion is reduced", async () => {
    const user = userEvent.setup();
    render(OverlayHarness, { kind: "sheet" });
    await user.click(screen.getByRole("button", { name: "Open" }));
    const sheet = screen.getByRole("dialog", { name: "Switch agent" });
    sheet.dispatchEvent(touch("touchstart", 100, 500));
    sheet.dispatchEvent(touch("touchmove", 100, 540));
    expect(sheet.style.transform).toBe("translateY(40px)");
    expect(sheet).toHaveAttribute("data-swiping");
    sheet.dispatchEvent(touch("touchcancel", 100, 540));
    expect(sheet.style.transform).toBe("");

    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("reduce") }));
    sheet.dispatchEvent(touch("touchstart", 100, 500));
    sheet.dispatchEvent(touch("touchmove", 100, 540));
    expect(sheet.style.transform).toBe("");
    sheet.dispatchEvent(touch("touchmove", 100, 700));
    sheet.dispatchEvent(touch("touchend", 100, 700));
    await act();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("leaves a drag up, or a scrolled sheet's drag down, to the page", async () => {
    const user = userEvent.setup();
    render(OverlayHarness, { kind: "sheet" });
    await user.click(screen.getByRole("button", { name: "Open" }));
    const sheet = sized(screen.getByRole("dialog", { name: "Switch agent" }), 400);
    drag(sheet, [100, 500], [100, 200]);
    await act();
    expect(sheet).toBeInTheDocument();
    const option = screen.getByRole("button", { name: "scout" });
    const body = option.parentElement;
    if (body === null) throw new Error("the sheet has no body");
    body.scrollTop = 40;
    drag(option, [100, 500], [100, 800]);
    await act();
    expect(sheet).toBeInTheDocument();
  });

  it("closes a drawer swiped left, not right", async () => {
    const user = userEvent.setup();
    render(OverlayHarness, { kind: "drawer" });
    await user.click(screen.getByRole("button", { name: "Open" }));
    const drawer = sized(screen.getByRole("dialog", { name: "Agents and places" }), 320);
    drag(drawer, [100, 300], [300, 300]);
    await act();
    expect(drawer).toBeInTheDocument();
    drag(drawer, [300, 300], [20, 310]);
    await act();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("ConfirmDialog", () => {
  it("starts on the safer choice: cancel for a danger action, confirm otherwise", () => {
    render(ConfirmDialog, {
      open: true,
      title: "Delete brittle?",
      confirmLabel: "Delete brittle",
      tone: "danger",
      onconfirm: () => {},
    });
    const dialog = screen.getByRole("alertdialog", { name: "Delete brittle?" });
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(dialog).toBeInTheDocument();
  });

  it("goes ahead from its confirm button, and counts Esc as cancel", async () => {
    const user = userEvent.setup();
    const onconfirm = vi.fn();
    const oncancel = vi.fn();
    const props = { title: "Restart atlas?", confirmLabel: "Restart", onconfirm, oncancel };
    const first = render(ConfirmDialog, { ...props, open: true });
    expect(screen.getByRole("button", { name: "Restart" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onconfirm).toHaveBeenCalledTimes(1);
    first.unmount();
    await settleHistory();

    render(ConfirmDialog, { ...props, open: true });
    await user.keyboard("{Escape}");
    expect(oncancel).toHaveBeenCalledTimes(1);
    expect(onconfirm).toHaveBeenCalledTimes(1);
  });
});

describe("confirmations", () => {
  it("asks through the host, one question at a time, in order", async () => {
    const user = userEvent.setup();
    render(ConfirmHost);
    const first = confirmations.ask({ title: "Stop atlas?", confirmLabel: "Stop atlas" });
    const second = confirmations.ask({ title: "Stop scout?", confirmLabel: "Stop scout" });
    await act();
    expect(screen.getByRole("alertdialog", { name: "Stop atlas?" })).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog", { name: "Stop scout?" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Stop atlas" }));
    await expect(first).resolves.toBe(true);
    await act();
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    await expect(second).resolves.toBe(false);
  });

  it("answers once the dialog is gone, so its history entry is already leaving", async () => {
    const user = userEvent.setup();
    render(ConfirmHost);
    const asked = confirmations.ask({ title: "Stop atlas?", confirmLabel: "Stop atlas" });
    await act();
    await user.click(screen.getByRole("button", { name: "Stop atlas" }));
    const answered = await asked;
    expect(answered).toBe(true);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("asks before losing unsaved work, listing what would be lost", async () => {
    const user = userEvent.setup();
    render(ConfirmHost);
    const leaving = confirmLeave(["Unsaved changes to SOUL.md", "Staged model settings"]);
    const dialog = await screen.findByRole("alertdialog", { name: "Discard unsaved changes?" });
    expect(dialog).toHaveTextContent("Unsaved changes to SOUL.md");
    expect(dialog).toHaveTextContent("Staged model settings");
    expect(screen.getByRole("button", { name: "Keep editing" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Keep editing" }));
    await expect(leaving).resolves.toBe(false);
  });

  it("counts Back as staying", async () => {
    render(ConfirmHost);
    const leaving = confirmLeave(["Unsaved changes to SOUL.md"]);
    await screen.findByRole("alertdialog");
    window.history.back();
    await expect(leaving).resolves.toBe(false);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
