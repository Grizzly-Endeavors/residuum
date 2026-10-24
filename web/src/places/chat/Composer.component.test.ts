import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { actionRegistry, type AppAction } from "../../lib/action-registry.svelte";
import { clearDraftAttachments, readDraft, saveDraft } from "../../lib/composer-drafts";
import type { StagedAttachment } from "../../lib/staged-attachments.svelte";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubPointer,
  stubWebSocket,
  type FetchHandler,
} from "../../test/component";
import Composer from "./Composer.svelte";
import { waitFor } from "../../test/wait";

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

type Props = Partial<{ replying: boolean; reconnecting: boolean; queued: number }>;

function composerWithRerender(props: Props = {}): Rendered & {
  rerender: (next: Props) => Promise<void>;
} {
  const onsend = vi.fn(() => true);
  const onstop = vi.fn();
  const { rerender } = render(Composer, {
    agent: "atlas",
    replying: false,
    reconnecting: false,
    queued: 0,
    onsend,
    onstop,
    ...props,
  });
  return {
    onsend,
    onstop,
    box: screen.getByRole("combobox", { name: "Message atlas" }),
    rerender: (next) => rerender(next),
  };
}

function composer(props: Props = {}): Rendered {
  const { onsend, onstop, box } = composerWithRerender(props);
  return { onsend, onstop, box };
}

function picker(): HTMLInputElement {
  const input = document.querySelector<HTMLInputElement>('input[type="file"]');
  if (input === null) throw new Error("no file picker");
  return input;
}

function pdf(name: string): File {
  return new File(["%PDF"], name, { type: "application/pdf" });
}

function uploaded(id: string, filename: string): Response {
  return jsonResponse({ id, filename, size: 4 });
}

/** Answer the hub's upload route with `answer`, and everything else (the model control's read) with 404. */
function answerUploads(answer: FetchHandler): void {
  mockFetch((url, init) =>
    url.startsWith("/api/hub/uploads") ? answer(url, init) : jsonResponse({}, 404),
  );
}

function attachedList(): HTMLElement {
  return screen.getByRole("list", { name: "Attached files" });
}

/** Whether the device reports a coarse pointer, as a touch screen does. */
let coarsePointer = false;

beforeEach(() => {
  coarsePointer = false;
  stubPointer(() => coarsePointer);
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
    action({ id: "chat:inbox", label: "New message to atlas", command: "inbox", takesText: true }),
    action({ id: "chat:stop", label: "Stop reply", command: "stop", disabled: "Not replying" }),
  ]);
});

afterEach(() => {
  unregister();
  summarize.mockReset();
  saveDraft("atlas", "");
  saveDraft("scout", "");
  clearDraftAttachments();
});

describe("sending", () => {
  it("sends with Enter, adds a line with Shift+Enter, and empties the box", async () => {
    const { onsend, box } = composer();
    await userEvent.type(box, "Check the wiki{Shift>}{Enter}{/Shift}index");
    expect(box).toHaveValue("Check the wiki\nindex");
    await userEvent.keyboard("{Enter}");
    expect(onsend).toHaveBeenCalledWith("Check the wiki\nindex", []);
    expect(box).toHaveValue("");
  });

  it("sends nothing while the box is empty", async () => {
    const { onsend, box } = composer();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await userEvent.type(box, "   {Enter}");
    expect(onsend).not.toHaveBeenCalled();
  });

  it("keeps the text and the files in the box when the line couldn't go", async () => {
    answerUploads(() => uploaded("upload_1", "notes.pdf"));
    const { onsend, box } = composer();
    onsend.mockReturnValue(false);
    await userEvent.upload(picker(), pdf("notes.pdf"));
    await waitFor(() => {
      expect(screen.queryByRole("progressbar")).toBeNull();
    });
    await userEvent.type(box, "/stop now{Enter}");
    expect(onsend).toHaveBeenCalledOnce();
    expect(box).toHaveValue("/stop now");
    expect(attachedList()).toHaveTextContent("notes.pdf");
    expect(readDraft("atlas")).toBe("/stop now");
  });

  it("is Stop while a reply runs and nothing is typed, and Send again once something is", async () => {
    const { onstop, box } = composer({ replying: true });
    await userEvent.click(screen.getByRole("button", { name: "Stop reply" }));
    expect(onstop).toHaveBeenCalledOnce();
    await userEvent.type(box, "Also check the index");
    expect(screen.queryByRole("button", { name: "Stop reply" })).toBeNull();
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });

  it("keeps one button for Send and Stop, so the focus has nothing to fall off", async () => {
    const { box } = composer({ replying: true });
    const stop = screen.getByRole("button", { name: "Stop reply" });
    expect(stop).toHaveAttribute("type", "button");
    await userEvent.type(box, "Also");
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).toBe(stop);
    expect(send).toHaveAttribute("type", "submit");
  });

  it("leaves focus in the message box after Send is pressed", async () => {
    const { onsend, box } = composer();
    await userEvent.type(box, "Check the wiki");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onsend).toHaveBeenCalledOnce();
    expect(box).toHaveFocus();
    expect(box).toHaveValue("");
  });

  it("puts focus back in the box when the keyboard presses Send, and after Stop", async () => {
    const { onsend, onstop, box } = composer({ replying: true });
    await userEvent.type(box, "Steer left");
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onsend).toHaveBeenCalledWith("Steer left", []);
    expect(box).toHaveFocus();

    await userEvent.click(screen.getByRole("button", { name: "Stop reply" }));
    expect(onstop).toHaveBeenCalledOnce();
    expect(box).toHaveFocus();
  });

  it("keeps the line, and the focus, when the line couldn't go", async () => {
    const { onsend, box } = composer();
    onsend.mockReturnValue(false);
    await userEvent.type(box, "/stop now");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(box).toHaveValue("/stop now");
    expect(box).toHaveFocus();
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
  it("is the message box's own combobox, named once", () => {
    const { box } = composer();
    expect(screen.getAllByRole("combobox")).toEqual([box]);
    expect(box.tagName).toBe("TEXTAREA");
    expect(box).toHaveAttribute("aria-expanded", "false");
    // Nothing to control while the list isn't there.
    expect(box).not.toHaveAttribute("aria-controls");
    expect(box).not.toHaveAttribute("aria-activedescendant");
    expect(screen.getByRole("button", { name: "Chat actions" })).not.toHaveAttribute(
      "aria-controls",
    );
  });

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

describe("files", () => {
  it("takes any kind of file from the paperclip, and uploads each at once", async () => {
    const posted: string[] = [];
    answerUploads((url) => {
      posted.push(url);
      return uploaded("upload_1", "build.log");
    });
    composer();
    await userEvent.click(screen.getByRole("button", { name: "Attach files" }));
    expect(picker()).not.toHaveAttribute("accept");

    await userEvent.upload(picker(), new File(["log"], "build.log", { type: "text/x-log" }));

    expect(attachedList()).toHaveTextContent("build.log");
    await waitFor(() => {
      expect(posted).toEqual(["/api/hub/uploads?filename=build.log"]);
    });
  });

  it("sends the uploaded files with the message, and empties the box", async () => {
    answerUploads(() => uploaded("upload_1", "notes.pdf"));
    const { onsend, box } = composer();
    await userEvent.upload(picker(), pdf("notes.pdf"));
    await waitFor(() => {
      expect(screen.queryByRole("progressbar")).toBeNull();
    });

    await userEvent.type(box, "Read this{Enter}");

    expect(onsend).toHaveBeenCalledOnce();
    const [text, attachments] = onsend.mock.calls[0] as [string, StagedAttachment[]];
    expect(text).toBe("Read this");
    expect(attachments.map((attachment) => attachment.uploadId)).toEqual(["upload_1"]);
    expect(box).toHaveValue("");
    expect(screen.queryByRole("list", { name: "Attached files" })).toBeNull();
  });

  it("sends a file with no text", async () => {
    answerUploads(() => uploaded("upload_1", "notes.pdf"));
    const { onsend } = composer();
    await userEvent.upload(picker(), pdf("notes.pdf"));
    await waitFor(() => {
      expect(screen.queryByRole("progressbar")).toBeNull();
    });
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onsend).toHaveBeenCalledWith("", [expect.objectContaining({ uploadId: "upload_1" })]);
  });

  it("holds Send while a file is uploading, says why, and lets it go once it's up", async () => {
    let finish: (response: Response) => void = () => {};
    answerUploads(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve;
        }),
    );
    const { onsend, box } = composer();
    await userEvent.upload(picker(), pdf("notes.pdf"));
    expect(screen.getByRole("progressbar", { name: "Uploading notes.pdf" })).toBeInTheDocument();

    await userEvent.type(box, "Read this{Enter}");
    expect(onsend).not.toHaveBeenCalled();
    expect(box).toHaveValue("Read this");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Wait for notes.pdf to finish uploading, then send.",
    );

    finish(uploaded("upload_1", "notes.pdf"));
    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
    await userEvent.keyboard("{Enter}");
    expect(onsend).toHaveBeenCalledOnce();
  });

  it("holds Send while a file failed, until it is tried again", async () => {
    let answer = (): Response => jsonResponse({ error: "disk full" }, 500);
    answerUploads(() => answer());
    const { onsend, box } = composer();
    await userEvent.upload(picker(), pdf("notes.pdf"));
    expect(await screen.findByText(/Couldn't upload notes\.pdf/)).toBeInTheDocument();

    await userEvent.type(box, "Read this{Enter}");
    expect(onsend).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "notes.pdf didn't upload. Try it again or remove it, then send.",
    );

    answer = () => uploaded("upload_2", "notes.pdf");
    await userEvent.click(screen.getByRole("button", { name: "Try again notes.pdf" }));
    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onsend).toHaveBeenCalledWith("Read this", [
      expect.objectContaining({ uploadId: "upload_2" }),
    ]);
  });

  it("removes a file, cancelling its upload", async () => {
    let signal: AbortSignal | undefined;
    answerUploads((_url, init) => {
      signal = init?.signal ?? undefined;
      return new Promise<Response>(() => {});
    });
    const { onsend, box } = composer();
    await userEvent.upload(picker(), pdf("notes.pdf"));
    await userEvent.click(screen.getByRole("button", { name: "Remove notes.pdf" }));

    expect(signal?.aborted).toBe(true);
    expect(screen.queryByRole("list", { name: "Attached files" })).toBeNull();
    await userEvent.type(box, "Just text{Enter}");
    expect(onsend).toHaveBeenCalledWith("Just text", []);
  });

  it("refuses a file over 50 MB in plain words, and uploads nothing", async () => {
    const posted: string[] = [];
    answerUploads((url) => {
      posted.push(url);
      return uploaded("upload_1", "film.mov");
    });
    composer();
    const film = pdf("film.mov");
    Object.defineProperty(film, "size", { value: 50 * 1024 * 1024 + 1 });
    await userEvent.upload(picker(), film);

    expect(screen.getByRole("status")).toHaveTextContent(
      "film.mov is larger than the 50 MB upload limit, so it wasn't attached.",
    );
    expect(screen.queryByRole("list", { name: "Attached files" })).toBeNull();
    expect(posted).toEqual([]);
  });

  it("takes pasted files", async () => {
    answerUploads(() => uploaded("upload_1", "shot.png"));
    const { box } = composer();
    await userEvent.click(box);
    await userEvent.paste({
      files: [new File(["png"], "shot.png", { type: "image/png" })],
    } as unknown as DataTransfer);
    expect(attachedList()).toHaveTextContent("shot.png");
    expect(box).toHaveValue("");
  });

  it("keeps the agent's files when the composer comes back", async () => {
    answerUploads(() => uploaded("upload_1", "notes.pdf"));
    const first = render(Composer, {
      agent: "atlas",
      replying: false,
      reconnecting: false,
      queued: 0,
      onsend: vi.fn(() => true),
      onstop: vi.fn(),
    });
    await userEvent.upload(picker(), pdf("notes.pdf"));
    first.unmount();

    composer();
    expect(attachedList()).toHaveTextContent("notes.pdf");
  });
});

describe("stopping the reply with Esc", () => {
  const HINT = "Press Esc again to stop";

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function user(): ReturnType<typeof userEvent.setup> {
    return userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
  }

  it("asks for a second Esc, announced politely, and stops on it", async () => {
    const { onstop, box } = composer({ replying: true });
    const keys = user();
    box.focus();

    await keys.keyboard("{Escape}");
    expect(screen.getByRole("status")).toHaveTextContent(HINT);
    expect(onstop).not.toHaveBeenCalled();

    await keys.keyboard("{Escape}");
    expect(onstop).toHaveBeenCalledOnce();
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("lets the hint go when two seconds pass, and the next Esc asks again", async () => {
    const { onstop, box } = composer({ replying: true });
    const keys = user();
    box.focus();

    await keys.keyboard("{Escape}");
    await vi.advanceTimersByTimeAsync(2000);
    expect(screen.queryByText(HINT)).toBeNull();
    await keys.keyboard("{Escape}");
    expect(onstop).not.toHaveBeenCalled();
    expect(screen.getByText(HINT)).toBeInTheDocument();
  });

  it("is disarmed by any other key", async () => {
    const { onstop, box } = composer({ replying: true });
    const keys = user();
    box.focus();

    await keys.keyboard("{Escape}a");
    expect(screen.queryByText(HINT)).toBeNull();
    await keys.keyboard("{Escape}");
    expect(onstop).not.toHaveBeenCalled();
  });

  it("is disarmed when focus leaves the composer", async () => {
    const { onstop, box } = composer({ replying: true });
    const keys = user();
    box.focus();
    await keys.keyboard("{Escape}");
    expect(screen.getByText(HINT)).toBeInTheDocument();

    box.blur();
    await settle();
    expect(screen.queryByText(HINT)).toBeNull();
    box.focus();
    await keys.keyboard("{Escape}");
    expect(onstop).not.toHaveBeenCalled();
  });

  it("is disarmed when the reply ends", async () => {
    const { rerender, box } = composerWithRerender({ replying: true });
    const keys = user();
    box.focus();
    await keys.keyboard("{Escape}");
    expect(screen.getByText(HINT)).toBeInTheDocument();

    await rerender({ replying: false });
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("counts a held key as one press", async () => {
    const { onstop, box } = composer({ replying: true });
    box.focus();
    for (const repeat of [false, true, true]) {
      box.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", repeat, bubbles: true, cancelable: true }),
      );
    }
    await settle();
    expect(onstop).not.toHaveBeenCalled();
    expect(screen.getByText(HINT)).toBeInTheDocument();
  });

  it("does nothing between replies", async () => {
    const { onstop, box } = composer({ replying: false });
    const keys = user();
    box.focus();
    await keys.keyboard("{Escape}{Escape}");
    expect(onstop).not.toHaveBeenCalled();
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("leaves Esc to the / menu, which closes without arming the stop", async () => {
    const { onstop, box } = composer({ replying: true });
    const keys = user();
    await keys.type(box, "/");
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    await keys.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(screen.queryByText(HINT)).toBeNull();
    expect(onstop).not.toHaveBeenCalled();
  });

  it("leaves Stop reply to stop at once", async () => {
    const { onstop } = composer({ replying: true });
    await user().click(screen.getByRole("button", { name: "Stop reply" }));
    expect(onstop).toHaveBeenCalledOnce();
  });
});

describe("Enter", () => {
  it("sends on a screen with a mouse, and leaves the key hint alone", async () => {
    const { onsend, box } = composer();
    expect(box).not.toHaveAttribute("enterkeyhint");
    await userEvent.type(box, "Hello{Enter}");
    expect(onsend).toHaveBeenCalledWith("Hello", []);
  });

  it("is a new line on a touch screen, where only Send sends", async () => {
    coarsePointer = true;
    const { onsend, box } = composer();
    expect(box).toHaveAttribute("enterkeyhint", "enter");
    await userEvent.type(box, "First{Enter}second");
    expect(box).toHaveValue("First\nsecond");
    expect(onsend).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onsend).toHaveBeenCalledWith("First\nsecond", []);
  });

  it("still lets Enter run the highlighted action of the / menu on a touch screen", async () => {
    coarsePointer = true;
    const { box } = composer();
    await userEvent.type(box, "/obs{Enter}");
    await settle();
    expect(summarize).toHaveBeenCalledOnce();
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
