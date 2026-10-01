import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { actionRegistry, type AppAction } from "../../lib/action-registry.svelte";
import { readDraft, saveDraft, saveDraftImages } from "../../lib/composer-drafts";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import Composer from "./Composer.svelte";

let unregister: () => void = () => {};
const summarize = vi.fn();

function action(overrides: Partial<AppAction> & Pick<AppAction, "id" | "label">): AppAction {
  return { group: "Actions", icon: "layers", run: vi.fn(), ...overrides };
}

interface Rendered {
  onsend: ReturnType<typeof vi.fn>;
  onstop: ReturnType<typeof vi.fn>;
  box: HTMLElement;
}

function composer(
  props: Partial<{ replying: boolean; reconnecting: boolean; queued: number }> = {},
): Rendered {
  const onsend = vi.fn();
  const onstop = vi.fn();
  render(Composer, {
    agent: "atlas",
    replying: false,
    reconnecting: false,
    queued: 0,
    onsend,
    onstop,
    ...props,
  });
  return { onsend, onstop, box: screen.getByRole("textbox", { name: "Message atlas" }) };
}

beforeEach(() => {
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: false,
    media,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  stubWebSocket();
  Element.prototype.scrollIntoView = vi.fn();
  // The model control's file read; its own tests are in ModelControl's.
  mockFetch(() => jsonResponse({}, 404));
  vi.spyOn(console, "error").mockImplementation(() => {});
  unregister = actionRegistry.register("test", () => [
    action({
      id: "chat:observe",
      label: "Summarize older messages now",
      command: "observe",
      run: summarize,
    }),
    action({ id: "chat:inbox", label: "Add a note", command: "inbox", takesText: true }),
    action({ id: "chat:stop", label: "Stop reply", command: "stop", disabled: "Not replying" }),
  ]);
});

afterEach(() => {
  unregister();
  summarize.mockReset();
  saveDraft("atlas", "");
  saveDraft("scout", "");
  saveDraftImages("atlas", []);
});

describe("sending", () => {
  it("sends with Enter, adds a line with Shift+Enter, and empties the box", async () => {
    const { onsend, box } = composer();
    await userEvent.type(box, "Check the wiki{Shift>}{Enter}{/Shift}index");
    expect(box).toHaveValue("Check the wiki\nindex");
    await userEvent.keyboard("{Enter}");
    expect(onsend).toHaveBeenCalledWith("Check the wiki\nindex", undefined);
    expect(box).toHaveValue("");
  });

  it("sends nothing while the box is empty", async () => {
    const { onsend, box } = composer();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await userEvent.type(box, "   {Enter}");
    expect(onsend).not.toHaveBeenCalled();
  });

  it("is Stop while a reply runs and nothing is typed, and Send again once something is", async () => {
    const { onstop, box } = composer({ replying: true });
    await userEvent.click(screen.getByRole("button", { name: "Stop reply" }));
    expect(onstop).toHaveBeenCalledOnce();
    await userEvent.type(box, "Also check the index");
    expect(screen.queryByRole("button", { name: "Stop reply" })).toBeNull();
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });
});

describe("drafts", () => {
  it("keeps what is typed for the agent until it is sent", async () => {
    const { box } = composer();
    await userEvent.type(box, "Half a thought");
    expect(readDraft("atlas")).toBe("Half a thought");
    await userEvent.keyboard("{Enter}");
    expect(readDraft("atlas")).toBe("");
  });

  it("starts from the agent's own draft", () => {
    saveDraft("atlas", "For atlas");
    saveDraft("scout", "For scout");
    const { box } = composer();
    expect(box).toHaveValue("For atlas");
  });
});

describe("the / menu", () => {
  it("is a combobox's list: / opens it, the arrows move along it, and Enter runs one", async () => {
    const { box } = composer();
    const field = screen.getByRole("combobox", { name: "Message atlas" });
    expect(field).toHaveAttribute("aria-expanded", "false");

    await userEvent.type(box, "/");
    const menu = screen.getByRole("listbox", { name: "Chat actions" });
    expect(field).toHaveAttribute("aria-expanded", "true");
    expect(field).toHaveAttribute("aria-controls", menu.id);
    expect(box).toHaveAttribute("aria-activedescendant", `${menu.id}-0`);
    await userEvent.keyboard("{ArrowDown}");
    expect(box).toHaveAttribute("aria-activedescendant", `${menu.id}-1`);
    await userEvent.keyboard("{ArrowUp}{Enter}");
    await settle();

    expect(summarize).toHaveBeenCalledOnce();
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(box).toHaveValue("");
  });

  it("narrows as the name is typed, fills it in with Tab, and closes with Esc", async () => {
    const { box } = composer();
    await userEvent.type(box, "/inb");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await userEvent.keyboard("{Tab}");
    expect(box).toHaveValue("/inbox ");
    expect(screen.queryByRole("listbox")).toBeNull();

    await userEvent.clear(box);
    await userEvent.type(box, "/");
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    box.dispatchEvent(escape);
    await settle();
    // Claimed, so the chat doesn't take it as Stop.
    expect(escape.defaultPrevented).toBe(true);
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("opens from its button with every action, and a disabled one does nothing", async () => {
    composer();
    await userEvent.click(screen.getByRole("button", { name: "Chat actions" }));
    expect(screen.getAllByRole("option")).toHaveLength(3);
    const stop = screen.getByRole("option", { name: /Stop reply/ });
    expect(stop).toHaveTextContent("Not replying");
    await userEvent.click(stop);
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Chat actions" }));
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

describe("images", () => {
  const png = (name = "shot.png"): File => new File(["png"], name, { type: "image/png" });

  it("attaches images to send, and each can be removed", async () => {
    const { onsend } = composer();
    const picker = document.querySelector<HTMLInputElement>('input[type="file"]');
    if (picker === null) throw new Error("no file picker");
    await userEvent.upload(picker, [png("a.png"), png("b.png")]);
    await vi.waitFor(() => {
      expect(screen.getAllByRole("img")).toHaveLength(2);
    });
    await userEvent.click(screen.getByRole("button", { name: "Remove image 2" }));
    expect(screen.getAllByRole("img")).toHaveLength(1);

    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onsend).toHaveBeenCalledWith("", [{ media_type: "image/png", data: "cG5n" }]);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("says why a file can't be attached", async () => {
    composer();
    const picker = document.querySelector<HTMLInputElement>('input[type="file"]');
    if (picker === null) throw new Error("no file picker");
    await userEvent.upload(picker, new File(["%PDF"], "notes.pdf", { type: "application/pdf" }), {
      applyAccept: false,
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "notes.pdf can't be attached. Attach a JPEG, PNG, GIF or WebP image.",
    );
  });
});

describe("while the connection is down", () => {
  it("says the messages wait for it", () => {
    composer({ reconnecting: true, queued: 2 });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Reconnecting — 2 messages will send once back online.",
    );
  });

  it("says so before anything is queued", () => {
    composer({ reconnecting: true });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Reconnecting — messages you send now will go out once back online.",
    );
  });
});
