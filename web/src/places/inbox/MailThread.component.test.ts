import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_UPLOAD_BYTES } from "../../lib/api";
import type { MailThread as Thread } from "../../lib/hub-types";
import { mail } from "../../lib/mail.svelte";
import { notifications } from "../../lib/notifications.svelte";
import { router } from "../../lib/router.svelte";
import { toast, type ToastAction } from "../../lib/toast.svelte";
import { render, screen, settle } from "../../test/component";
import {
  agentMessage,
  jsonResponse,
  mailThread,
  resetMail,
  serveMail,
  userMessage,
  type SeenRequest,
} from "../../test/mail";
import { waitFor } from "../../test/wait";
import MailThread from "./MailThread.svelte";

const ID = "atlas/agent/20260311_wiki_audit";

const AUDIT_PDF = {
  filename: "source-notes.pdf",
  size: 2048,
  content_type: "application/pdf",
  url: "/api/agents/scout/inbox/20260311_scout/attachments/0",
  agent: "scout",
  path: "inbox/user/attachments/20260311_scout/source-notes.pdf",
};

const SCOPE_MD = {
  filename: "audit-scope.md",
  size: 512,
  content_type: "text/markdown",
  url: "/api/agents/atlas/inbox/20260311_wiki_audit/attachments/0",
  agent: "atlas",
  path: "inbox/agent/attachments/20260311_wiki_audit/audit-scope.md",
};

function audit(overrides: Partial<Thread> = {}): Thread {
  return mailThread(
    ID,
    [
      userMessage(ID, {
        to: ["atlas", "scout"],
        at: "2026-03-11T14:05:00Z",
        body: "Please check the **stale pages**.",
        attachments: [SCOPE_MD],
        seen_by: [
          { agent: "atlas", seen: true },
          { agent: "scout", seen: false },
        ],
      }),
      agentMessage("atlas/user/20260311_atlas", { body: "On it." }),
      agentMessage("scout/user/20260311_scout", {
        body: "Notes attached.",
        attachments: [AUDIT_PDF],
      }),
    ],
    overrides,
  );
}

let requests: SeenRequest[] = [];
let answer: (request: SeenRequest) => Response | Promise<Response> | undefined;

function serve(current: () => Thread): void {
  answer = () => undefined;
  requests = serveMail((request) => {
    const own = answer(request);
    if (own !== undefined) return own;
    if (request.method === "GET" && request.path.startsWith("/api/hub/mail/threads/")) {
      return jsonResponse(current());
    }
    if (request.path.startsWith("/api/hub/mail/threads?")) {
      return jsonResponse({ threads: [], next: null });
    }
    return undefined;
  });
}

/** Open the thread as the place does, through the store, and show it. */
function show(onback = vi.fn()): ReturnType<typeof vi.fn> {
  void mail.open(ID);
  render(MailThread, { threadId: ID, onback });
  return onback;
}

function posted(suffix: string): SeenRequest[] {
  return requests.filter(
    (request) =>
      request.method === "POST" && request.path === `/api/hub/mail/threads/${ID}${suffix}`,
  );
}

beforeEach(() => {
  resetMail();
  serve(() => audit());
});

// The store is reset before each test, not after: the views are still
// mounted until the shared cleanup, and would open their thread again.
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("an open thread", () => {
  it("shows the subject and every message, oldest first, with who wrote to whom", async () => {
    show();
    expect(await screen.findByRole("heading", { name: "Wiki audit", level: 2 })).toBeTruthy();
    expect(screen.getByText("You and atlas, scout · 3 messages")).toBeTruthy();
    const messages = screen.getAllByRole("article");
    expect(messages).toHaveLength(3);
    expect(messages[0]?.textContent).toContain("You");
    expect(messages[0]?.textContent).toContain("to atlas and scout");
    expect(messages[1]?.textContent).toContain("atlas");
    expect(messages[1]?.textContent).not.toContain("to ");
    expect(messages[2]?.textContent).toContain("Notes attached.");
    expect(screen.getByText("stale pages").tagName).toBe("STRONG");
    const time = messages[0]?.querySelector("time");
    expect(time?.getAttribute("datetime")).toBe("2026-03-11T14:05:00Z");
  });

  it("shows a loading state while the thread is fetched", () => {
    answer = () => new Promise<Response>(() => {});
    show();
    expect(screen.getByText("Loading the conversation")).toBeTruthy();
  });

  it("says when the thread can't be fetched, and Try again fetches it", async () => {
    let fail = true;
    answer = (request) =>
      fail && request.method === "GET" && request.path.startsWith("/api/hub/mail/threads/")
        ? jsonResponse({ error: "boom" }, 500)
        : undefined;
    show();
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Couldn't open that conversation.",
    );
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "Wiki audit" })).toBeTruthy();
  });

  it("marks your messages with who has seen them", async () => {
    show();
    await screen.findByRole("heading", { name: "Wiki audit" });
    expect(screen.getByText("Seen by atlas, not yet by scout")).toBeTruthy();
    expect(screen.getAllByText(/Seen by|Not seen yet/)).toHaveLength(1);
  });

  it("doesn't show another thread the store still holds", async () => {
    serve(() => mailThread("scout/user/other", [agentMessage("scout/user/other")]));
    void mail.open("scout/user/other");
    render(MailThread, { threadId: ID, onback: vi.fn() });
    await settle();
    expect(screen.queryByRole("heading", { name: "Wiki audit" })).toBeNull();
    expect(screen.getByText("Loading the conversation")).toBeTruthy();
  });

  it("goes back to the list from Back", async () => {
    const onback = show();
    await userEvent.click(await screen.findByRole("button", { name: "Back to list" }));
    expect(onback).toHaveBeenCalledOnce();
  });

  it("marks an unread thread read when it opens", async () => {
    serve(() => audit({ messages: [agentMessage("atlas/user/x", { read: false })] }));
    show();
    await screen.findByRole("heading", { name: "Wiki audit" });
    await waitFor(() => {
      expect(posted("/read")).toHaveLength(1);
    });
  });
});

describe("attachments", () => {
  it("opens a Markdown file beside the thread, in the agent's workspace", async () => {
    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Open audit-scope.md" }));
    expect(openPanel).toHaveBeenCalledWith({
      kind: "file",
      path: SCOPE_MD.path,
      agent: "atlas",
    });
  });

  it("downloads any other file", async () => {
    show();
    const link = await screen.findByRole("link", { name: "Download source-notes.pdf" });
    expect(link.getAttribute("href")).toBe(AUDIT_PDF.url);
    expect(link.getAttribute("download")).toBe("source-notes.pdf");
    expect(screen.queryByRole("button", { name: "Open source-notes.pdf" })).toBeNull();
  });
});

describe("archive and restore", () => {
  it("archives the thread, goes back, and Undo moves it to the inbox again", async () => {
    let action: ToastAction | undefined;
    const success = vi.spyOn(toast, "success").mockImplementation((_message, given) => {
      action = given;
      return 0;
    });
    const onback = show();
    await userEvent.click(await screen.findByRole("button", { name: "Archive" }));
    await waitFor(() => {
      expect(onback).toHaveBeenCalledOnce();
    });
    expect(posted("/archive")).toHaveLength(1);
    expect(success).toHaveBeenCalledWith("Archived “Wiki audit”.", expect.anything());
    expect(action?.label).toBe("Undo");
    action?.onClick();
    await waitFor(() => {
      expect(posted("/restore")).toHaveLength(1);
    });
  });

  it("offers Move to Inbox on an archived thread, with Undo archiving it again", async () => {
    let action: ToastAction | undefined;
    const success = vi.spyOn(toast, "success").mockImplementation((_message, given) => {
      action = given;
      return 0;
    });
    serve(() =>
      audit({ messages: [agentMessage("atlas/user/x", { archived: true, body: "Done." })] }),
    );
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Move to Inbox" }));
    await waitFor(() => {
      expect(posted("/restore")).toHaveLength(1);
    });
    expect(success).toHaveBeenCalledWith(
      "Moved “Wiki audit” back to the inbox.",
      expect.anything(),
    );
    action?.onClick();
    await waitFor(() => {
      expect(posted("/archive")).toHaveLength(1);
    });
  });

  it("offers neither on a thread no agent wrote in", async () => {
    serve(() => audit({ messages: [userMessage(ID)] }));
    show();
    await screen.findByRole("heading", { name: "Wiki audit" });
    expect(screen.queryByRole("button", { name: "Archive" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Move to Inbox" })).toBeNull();
  });

  it("says when archiving fails and stays on the thread, and Try again archives it", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    let fail = true;
    answer = (request) =>
      fail && request.path.endsWith("/archive") ? jsonResponse({ error: "nope" }, 500) : undefined;
    const onback = show();
    await userEvent.click(await screen.findByRole("button", { name: "Archive" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Couldn't archive it.");
    expect(onback).not.toHaveBeenCalled();
    fail = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => {
      expect(onback).toHaveBeenCalledOnce();
    });
  });

  it("surfaces a failed Undo as a notification", async () => {
    let action: ToastAction | undefined;
    vi.spyOn(toast, "success").mockImplementation((_message, given) => {
      action = given;
      return 0;
    });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    answer = (request) =>
      request.path.endsWith("/restore") ? jsonResponse({ error: "nope" }, 500) : undefined;
    show();
    await userEvent.click(await screen.findByRole("button", { name: "Archive" }));
    await waitFor(() => {
      expect(action).toBeDefined();
    });
    action?.onClick();
    await waitFor(() => {
      expect(surface).toHaveBeenCalledWith("error", expect.stringContaining("Couldn't move it"));
    });
  });
});

describe("the reply", () => {
  it("is addressed to everyone in the thread and says when they will see it", async () => {
    show();
    const form = await screen.findByRole("form", { name: "Reply to the thread" });
    expect(form.textContent).toContain("To: atlas and scout");
    expect(form.textContent).toContain("They’ll see this next time they check their inbox.");
  });

  it("states the upload limit before anything is attached", async () => {
    show();
    const form = await screen.findByRole("form", { name: "Reply to the thread" });
    expect(form).toHaveTextContent(
      "Any file, up to 50 MB each. They’ll see this next time they check their inbox.",
    );
  });

  it("sends from the text box on Ctrl+Enter and on Cmd+Enter", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    answer = (request) =>
      request.path.endsWith("/reply")
        ? jsonResponse({
            message: userMessage(`atlas/agent/new${String(posted("/reply").length)}`, {
              seen_by: [],
            }),
          })
        : undefined;
    show();
    const reply = await screen.findByRole("textbox", { name: "Reply" });
    await userEvent.type(reply, "Thanks.{Control>}{Enter}{/Control}");
    await waitFor(() => {
      expect(posted("/reply")).toHaveLength(1);
    });
    expect(posted("/reply")[0]?.body).toEqual({ body: "Thanks.", attachments: [] });

    await userEvent.type(reply, "Again.{Meta>}{Enter}{/Meta}");
    await waitFor(() => {
      expect(posted("/reply")).toHaveLength(2);
    });
    expect(posted("/reply")[1]?.body).toEqual({ body: "Again.", attachments: [] });
  });

  it("can't be sent while blank, and sends what is written to everyone", async () => {
    const success = vi.spyOn(toast, "success").mockImplementation(() => 0);
    answer = (request) =>
      request.path.endsWith("/reply")
        ? jsonResponse({
            message: userMessage("atlas/agent/new", {
              to: ["atlas", "scout"],
              body: "Thanks both.",
              seen_by: [],
            }),
          })
        : undefined;
    show();
    const send = await screen.findByRole("button", { name: "Send" });
    expect((send as HTMLButtonElement).disabled).toBe(true);
    await userEvent.type(screen.getByRole("textbox", { name: "Reply" }), "Thanks both.");
    expect((send as HTMLButtonElement).disabled).toBe(false);
    await userEvent.click(send);
    await waitFor(() => {
      expect(screen.getAllByRole("article")).toHaveLength(4);
    });
    expect(posted("/reply")[0]?.body).toEqual({ body: "Thanks both.", attachments: [] });
    expect(success).toHaveBeenCalledWith("Sent to atlas and scout.");
    expect(screen.getByRole("textbox", { name: "Reply" })).toHaveProperty("value", "");
  });

  it("keeps the text and says why when the send fails", async () => {
    answer = (request) =>
      request.path.endsWith("/reply") ? jsonResponse({ error: "hub is down" }, 500) : undefined;
    show();
    await userEvent.type(await screen.findByRole("textbox", { name: "Reply" }), "Still there?");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Couldn't send your reply.");
    expect(screen.getByRole("textbox", { name: "Reply" })).toHaveProperty("value", "Still there?");
  });

  it("keeps the draft when the view is shown again", async () => {
    void mail.open(ID);
    const first = render(MailThread, { threadId: ID, onback: vi.fn() });
    await userEvent.type(await screen.findByRole("textbox", { name: "Reply" }), "Half a thought");
    first.unmount();
    show();
    expect(await screen.findByRole("textbox", { name: "Reply" })).toHaveProperty(
      "value",
      "Half a thought",
    );
  });
});

describe("attaching files to the reply", () => {
  function file(name: string, size = 10): File {
    const picked = new File(["x"], name, { type: "text/plain" });
    Object.defineProperty(picked, "size", { value: size });
    return picked;
  }

  async function attach(...files: File[]): Promise<void> {
    await userEvent.upload(screen.getByLabelText("Files to attach"), files);
  }

  it("shows a file uploading, and sends once it is up", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    let finish: (response: Response) => void = () => {};
    answer = (request) => {
      if (request.path.startsWith("/api/hub/uploads")) {
        return new Promise<Response>((resolve) => {
          finish = resolve;
        });
      }
      if (request.path.endsWith("/reply")) {
        return jsonResponse({ message: userMessage("atlas/agent/new", { seen_by: [] }) });
      }
      return undefined;
    };
    show();
    const form = await screen.findByRole("form", { name: "Reply to the thread" });
    // The notice has a live region to land in before it is needed, so it is spoken when it shows.
    const live = form.querySelector("[aria-live=polite]");
    expect(live?.textContent).toBe("");
    await attach(file("log.txt"));
    expect(screen.getByRole("progressbar", { name: "Uploading log.txt" })).toBeTruthy();
    await userEvent.type(screen.getByRole("textbox", { name: "Reply" }), "Log attached.");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await settle();
    expect(posted("/reply")).toHaveLength(0);
    expect(live).toHaveTextContent("Sending once the files finish uploading.");
    expect(form.querySelector("[aria-live=polite]")).toBe(live);
    finish(jsonResponse({ id: "upload_0001", filename: "log.txt", size: 10 }));
    await waitFor(() => {
      expect(posted("/reply")).toHaveLength(1);
    });
    expect(posted("/reply")[0]?.body).toEqual({
      body: "Log attached.",
      attachments: ["upload_0001"],
    });
  });

  it("shows a failed upload, tries it again, and removes a file", async () => {
    let uploads = 0;
    answer = (request) => {
      if (!request.path.startsWith("/api/hub/uploads")) return undefined;
      uploads += 1;
      return uploads === 1
        ? jsonResponse({ error: "disk full" }, 500)
        : jsonResponse({ id: "upload_0002", filename: "log.txt", size: 10 });
    };
    show();
    await screen.findByRole("form", { name: "Reply to the thread" });
    await attach(file("log.txt"));
    expect(await screen.findByText(/Couldn't upload log\.txt\./)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Try again log.txt" }));
    await waitFor(() => {
      expect(screen.queryByText(/Couldn't upload log\.txt\./)).toBeNull();
    });
    expect(uploads).toBe(2);
    await userEvent.click(screen.getByRole("button", { name: "Remove log.txt" }));
    expect(screen.queryByRole("list", { name: "Attached files" })).toBeNull();
  });

  it("refuses a file over the upload limit before uploading it", async () => {
    show();
    await screen.findByRole("form", { name: "Reply to the thread" });
    await attach(file("huge.mov", MAX_UPLOAD_BYTES + 1));
    expect(
      screen.getByText("huge.mov is larger than the 50 MB upload limit, so it wasn't attached."),
    ).toBeTruthy();
    expect(requests.some((request) => request.path.startsWith("/api/hub/uploads"))).toBe(false);
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/huge\.mov/)).toBeNull();
  });
});
