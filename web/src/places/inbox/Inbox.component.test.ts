import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { hub } from "../../lib/hub.svelte";
import type {
  AgentOverview,
  AgentSummary,
  MailFolder,
  MailThread,
  MailThreadPage,
} from "../../lib/hub-types";
import { mail } from "../../lib/mail.svelte";
import { overview } from "../../lib/overview.svelte";
import { router } from "../../lib/router.svelte";
import { HOME, INBOX, locationAt, type InboxPlace, type Panel, type Place } from "../../lib/routes";
import { render, screen, settle, stubWebSocket } from "../../test/component";
import {
  agentMessage,
  jsonResponse,
  mailSummary,
  mailThread,
  resetMail,
  serveMail,
  type SeenRequest,
} from "../../test/mail";
import { waitFor } from "../../test/wait";
import Inbox from "./Inbox.svelte";

const ATTACHMENT_PATH = "inbox/user/attachments/mock_1/notes.md";
const SENT_THREAD = "atlas/agent/20260314_hello";

function threadOf(id: string): MailThread {
  const agent = id.split("/")[0] ?? "atlas";
  return mailThread(
    id,
    [
      agentMessage(id, {
        title: `Subject of ${id}`,
        attachments: [
          {
            filename: "notes.md",
            size: 12,
            content_type: "text/markdown",
            url: `/api/agents/${agent}/inbox/x/attachments/0`,
            agent,
            path: ATTACHMENT_PATH,
          },
        ],
      }),
    ],
    { subject: `Subject of ${id}`, participants: [agent], recipients_for_reply: [agent] },
  );
}

function agentSummary(name: string): AgentSummary {
  return { name, state: "running" } as AgentSummary;
}

function overviewOf(name: string, unread: number): AgentOverview {
  return {
    name,
    last_message: null,
    live_sessions: [],
    upcoming: [],
    inbox_unread: unread,
    outbound_problems: [],
  };
}

interface Server {
  /** The list each folder answers with. */
  folders: Partial<Record<MailFolder, MailThreadPage>>;
  /** Threads by every ref that opens them. */
  threads: Record<string, MailThread>;
  failList: boolean;
  /** Hold list requests until released, to see the loading state. */
  holdList: boolean;
  /** Answer thread requests only once this settles. */
  threadGate: Promise<void> | null;
  requests: SeenRequest[];
}

/** A promise settled from outside, to hold a request until the test lets it go. */
function gate(): { promise: Promise<void>; release: () => void } {
  let release: () => void = () => {};
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

function serve(initial: Partial<Omit<Server, "requests">> = {}): Server {
  const server: Server = {
    folders: {},
    threads: {},
    failList: false,
    holdList: false,
    threadGate: null,
    requests: [],
    ...initial,
  };
  server.requests = serveMail((request) => {
    if (request.path.startsWith("/api/hub/mail/threads?")) {
      if (server.holdList) return new Promise<Response>(() => {});
      if (server.failList) return jsonResponse({ error: "boom" }, 500);
      const query = new URLSearchParams(request.path.slice(request.path.indexOf("?")));
      const folder = (query.get("folder") ?? "inbox") as MailFolder;
      return jsonResponse(server.folders[folder] ?? { threads: [], next: null });
    }
    if (request.method === "GET" && request.path.startsWith("/api/hub/mail/threads/")) {
      const thread = server.threads[request.path.slice("/api/hub/mail/threads/".length)];
      const answer = (): Response =>
        thread === undefined
          ? jsonResponse({ error: "There is no such thread." }, 404)
          : jsonResponse(thread);
      return server.threadGate === null ? answer() : server.threadGate.then(answer);
    }
    if (request.path === "/api/hub/mail/send") return jsonResponse({ thread_id: SENT_THREAD });
    return undefined;
  });
  return server;
}

const listRequests = (server: Server): string[] =>
  server.requests
    .filter((r) => r.method === "GET" && r.path.startsWith("/api/hub/mail/threads?"))
    .map((r) => `${r.method} ${r.path}`);

function inbox(overrides: Partial<InboxPlace> = {}): InboxPlace {
  return { ...INBOX, ...overrides };
}

/** Put the router where the place is, with `panel` open, as the app would be. */
function at(place: InboxPlace, panel: Panel | null = null): void {
  router.location = { ...locationAt(place), panel };
}

interface NavigationSpies {
  open: MockInstance<typeof router.openPlace>;
  replace: MockInstance<typeof router.replacePlace>;
  closeItem: MockInstance<typeof router.closeItem>;
  closePanel: MockInstance<typeof router.closePanel>;
}

function spyNavigation(): NavigationSpies {
  return {
    open: vi.spyOn(router, "openPlace").mockResolvedValue(true),
    replace: vi.spyOn(router, "replacePlace").mockResolvedValue(true),
    closeItem: vi.spyOn(router, "closeItem").mockResolvedValue(true),
    closePanel: vi.spyOn(router, "closePanel").mockResolvedValue(true),
  };
}

type View = ReturnType<typeof render<typeof Inbox>>;

function show(place: InboxPlace, panel: Panel | null = null): View {
  at(place, panel);
  return render(Inbox, { place });
}

/** Let the spied navigation take effect as the router does: the address moves and the place re-renders. */
function followNavigation(nav: NavigationSpies, view: View): void {
  const go = async (next: Place): Promise<boolean> => {
    if (next.kind !== "inbox") return true;
    at(next);
    await view.rerender({ place: next });
    return true;
  };
  nav.open.mockImplementation(go);
  nav.replace.mockImplementation(go);
}

/** The list column, which the narrow layout hides (jsdom runs no container queries). */
function listColumn(): HTMLElement {
  return screen.getByRole("region", { name: "Conversations" });
}

function chosen(name: string): boolean {
  return (
    screen.getByRole("button", { name: new RegExp(`^${name}`) }).getAttribute("aria-pressed") ===
    "true"
  );
}

const DEPLOY = mailSummary("atlas/user/mock_1", { unread: true, subject: "Deploy tomorrow" });
const DIGEST = mailSummary("scout/user/mock_2", { subject: "", has_attachments: true });
const INBOX_PAGE: MailThreadPage = { threads: [DEPLOY, DIGEST], next: null };

/** Open DEPLOY from the address, once its thread and the list have arrived. */
async function openDeploy(panel: Panel | null = null): Promise<NavigationSpies> {
  serve({
    folders: { inbox: INBOX_PAGE },
    threads: { [DEPLOY.thread_id]: threadOf(DEPLOY.thread_id) },
  });
  const nav = spyNavigation();
  show(inbox({ thread: DEPLOY.thread_id }), panel);
  await waitFor(() => {
    expect(mail.thread?.thread_id).toBe(DEPLOY.thread_id);
  });
  await screen.findByRole("link", { name: /Deploy tomorrow/ });
  return nav;
}

beforeEach(() => {
  resetMail();
  stubWebSocket();
  Element.prototype.scrollIntoView = vi.fn();
  hub.agents = [agentSummary("atlas"), agentSummary("scout")];
  overview.overviews = { atlas: overviewOf("atlas", 2), scout: overviewOf("scout", 1) };
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Inbox", () => {
  it("lists the folder's conversations under the place header", async () => {
    const server = serve({ folders: { inbox: INBOX_PAGE } });
    show(inbox());

    expect(screen.getByRole("heading", { level: 1, name: "Inbox" })).toBeTruthy();
    expect(screen.getByText("Mail between you and your agents")).toBeTruthy();
    const deploy = await screen.findByRole("link", { name: /Deploy tomorrow/ });
    expect(deploy).toHaveAttribute("href", "/inbox?thread=atlas%2Fuser%2Fmock_1");
    expect(deploy).toHaveTextContent("Deploy tomorrow, unread");
    expect(screen.getByRole("link", { name: /\(no subject\)/ })).toHaveTextContent(
      "has attachments",
    );
    expect(listRequests(server)).toEqual(["GET /api/hub/mail/threads?folder=inbox&limit=50"]);
  });

  it("names the agents a Sent row went to, and an Inbox row's agents plainly", async () => {
    const wrote = mailSummary("atlas/user/mock_3", {
      subject: "Plan",
      participants: ["atlas", "scout"],
      in_inbox: false,
      in_sent: true,
    });
    serve({ folders: { inbox: INBOX_PAGE, sent: { threads: [wrote], next: null } } });
    show(inbox({ folder: "sent" }));
    expect(await screen.findByRole("link", { name: /Plan/ })).toHaveTextContent("To atlas, scout");
  });

  it("shows each folder as a link, the Inbox with the unread count", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const nav = spyNavigation();
    show(inbox());
    const folders = screen.getByRole("navigation", { name: "Folders" });
    const current = folders.querySelector("[aria-current=page]");
    expect(current).toHaveTextContent("Inbox");
    expect(current).toHaveTextContent("3 unread");
    expect(screen.getByRole("link", { name: "Sent" })).toHaveAttribute(
      "href",
      "/inbox?folder=sent",
    );

    await userEvent.click(screen.getByRole("link", { name: "Archived" }));
    expect(nav.replace).toHaveBeenCalledWith(inbox({ folder: "archived" }), {});
  });

  it("fetches the folder and filter the place names", async () => {
    const server = serve();
    show(inbox({ folder: "sent", agent: "scout" }));
    await settle();
    expect(listRequests(server)).toEqual([
      "GET /api/hub/mail/threads?folder=sent&agent=scout&limit=50",
    ]);
  });

  it("filters to one agent from the select, which shows each agent's unread count", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const nav = spyNavigation();
    show(inbox());
    const select = screen.getByRole("combobox", { name: "Show conversations with" });
    expect(screen.getByRole("option", { name: "atlas (2 unread)" })).toBeTruthy();

    await userEvent.selectOptions(select, "scout");
    expect(nav.replace).toHaveBeenCalledWith(inbox({ agent: "scout" }), {});
  });

  it("shows a skeleton until the first page arrives", async () => {
    serve({ holdList: true });
    show(inbox());
    await settle();
    expect(screen.getByRole("status")).toHaveTextContent("Loading conversations");
  });

  it("says when the list couldn't load, and Try again fetches it again", async () => {
    const server = serve({ failList: true });
    show(inbox());
    expect(await screen.findByText(/Couldn't load your inbox/)).toBeTruthy();

    server.failList = false;
    server.folders.inbox = INBOX_PAGE;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("link", { name: /Deploy tomorrow/ })).toBeTruthy();
    expect(listRequests(server)).toHaveLength(2);
  });

  it.each<[MailFolder, string | null, string]>([
    ["inbox", null, "You're all caught up."],
    ["inbox", "scout", "Nothing from scout in your inbox."],
    ["sent", null, "You haven't sent anything yet."],
    ["sent", "scout", "You haven't written to scout yet."],
    ["archived", null, "Nothing is archived."],
    ["archived", "scout", "Nothing with scout is archived."],
  ])("says what an empty %s list for %s means", async (folder, agent, words) => {
    serve();
    show(inbox({ folder, agent }));
    expect(await screen.findByText(new RegExp(words.replace(/[.()]/g, "\\$&")))).toBeTruthy();
  });

  it("offers New message from an empty Sent folder", async () => {
    serve();
    const nav = spyNavigation();
    show(inbox({ folder: "sent", agent: "scout" }));
    await screen.findByText(/You haven't written to scout yet/);
    const [, fromEmpty] = screen.getAllByRole("button", { name: "New message" });
    expect(fromEmpty).toBeDefined();
    if (fromEmpty) await userEvent.click(fromEmpty);
    expect(nav.open).toHaveBeenCalledWith(
      inbox({ folder: "sent", agent: "scout", compose: { to: ["scout"] } }),
      {},
    );
  });

  it("pages older threads with Show older, and offers Try again when that fails", async () => {
    const server = serve({ folders: { inbox: { threads: [DEPLOY], next: "1:x" } } });
    show(inbox());
    await screen.findByRole("link", { name: /Deploy tomorrow/ });

    server.failList = true;
    await userEvent.click(screen.getByRole("button", { name: "Show older" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't load older conversations");

    server.failList = false;
    server.folders.inbox = { threads: [DIGEST], next: null };
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("link", { name: /\(no subject\)/ })).toBeTruthy();
    expect(listRequests(server).at(-1)).toBe(
      "GET /api/hub/mail/threads?folder=inbox&before=1%3Ax&limit=50",
    );
    expect(screen.queryByRole("button", { name: "Show older" })).toBeNull();
  });

  it("opens a thread from the list by pushing, and switches threads by replacing", async () => {
    serve({
      folders: { inbox: INBOX_PAGE },
      threads: { [DEPLOY.thread_id]: threadOf(DEPLOY.thread_id) },
    });
    const nav = spyNavigation();
    const view = show(inbox());
    await userEvent.click(await screen.findByRole("link", { name: /Deploy tomorrow/ }));
    expect(nav.open).toHaveBeenCalledWith(inbox({ thread: DEPLOY.thread_id }), {});

    const opened = inbox({ thread: DEPLOY.thread_id });
    at(opened);
    await view.rerender({ place: opened });
    await waitFor(() => {
      expect(screen.getByRole("link", { name: /Deploy tomorrow/ })).toHaveAttribute(
        "aria-current",
        "true",
      );
    });
    expect(screen.getByRole("region", { name: "Conversation" })).toBeTruthy();

    await userEvent.click(screen.getByRole("link", { name: /\(no subject\)/ }));
    expect(nav.replace).toHaveBeenCalledWith(inbox({ thread: DIGEST.thread_id }), {});
  });

  it("names the thread by its own id once it loads from a member's ref", async () => {
    const canonical = "atlas/agent/20260311_wiki_audit";
    const member = "scout/agent/20260311_wiki_audit";
    const thread = threadOf(canonical);
    serve({ threads: { [member]: thread, [canonical]: thread } });
    const nav = spyNavigation();
    show(inbox({ thread: member }));
    await waitFor(() => {
      expect(nav.replace).toHaveBeenCalledWith(inbox({ thread: canonical }), {});
    });
  });

  it("keeps a panel open on the address it rewrites", async () => {
    const canonical = "atlas/agent/20260311_wiki_audit";
    const member = "scout/agent/20260311_wiki_audit";
    const thread = threadOf(canonical);
    serve({ threads: { [member]: thread, [canonical]: thread } });
    const nav = spyNavigation();
    const panel: Panel = { kind: "file", agent: "atlas", path: ATTACHMENT_PATH };
    show(inbox({ thread: member }), panel);
    await waitFor(() => {
      expect(nav.replace).toHaveBeenCalledWith(inbox({ thread: canonical }), { panel });
    });
  });

  it("says a missing thread isn't there and offers the way back to the list", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const nav = spyNavigation();
    show(inbox({ thread: "atlas/user/gone" }));
    expect(
      await screen.findByRole("heading", { name: "That conversation isn't there any more" }),
    ).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
    expect(nav.closeItem).toHaveBeenCalledTimes(1);
    expect(nav.closePanel).not.toHaveBeenCalled();
  });

  it("opens a New message with the filtered agent as its recipient", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const nav = spyNavigation();
    show(inbox({ agent: "atlas" }));
    await userEvent.click(screen.getByRole("button", { name: "New message" }));
    expect(nav.open).toHaveBeenCalledWith(
      inbox({ agent: "atlas", compose: { to: ["atlas"] } }),
      {},
    );
  });

  it("starts the store's draft for a New message and opens the sent thread in Sent", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const nav = spyNavigation();
    show(inbox({ compose: { to: ["atlas"] } }));
    expect(screen.getByRole("form", { name: "New message" })).toBeTruthy();
    const draft = mail.newMessage;
    expect(draft?.to).toEqual(["atlas"]);
    if (draft) draft.body = "Hello there";

    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect(nav.replace).toHaveBeenCalledWith(inbox({ folder: "sent", thread: SENT_THREAD }), {});
    });
  });

  it("asks before leaving an unsent New message, and drops the draft once it closes", async () => {
    serve({ folders: { inbox: INBOX_PAGE } });
    const place = inbox({ compose: { to: ["atlas"] } });
    const view = show(place);
    const draft = mail.newMessage;
    if (draft) draft.body = "Half a thought";

    expect(router.guard.losses(locationAt(HOME))).toEqual(["An unsent new message"]);
    expect(router.guard.losses(locationAt(place))).toEqual([]);

    await view.rerender({ place: inbox() });
    expect(mail.newMessage).toBeNull();

    // Once the Inbox is gone, its check no longer answers for the store.
    view.unmount();
    mail.openNewMessage().body = "Again";
    expect(router.guard.losses(locationAt(HOME))).toEqual([]);
    mail.discardNewMessage();
  });

  describe("a New message opened for other agents while one is open", () => {
    it("adds them to a draft with text and keeps the text", async () => {
      serve();
      const view = show(inbox({ compose: { to: ["atlas"] } }));
      await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Half a thought");

      const next = inbox({ compose: { to: ["scout"] } });
      at(next);
      await view.rerender({ place: next });

      expect(chosen("atlas")).toBe(true);
      expect(chosen("scout")).toBe(true);
      expect(screen.getByRole("textbox", { name: /^Message/ })).toHaveProperty(
        "value",
        "Half a thought",
      );
    });

    it("re-addresses an empty draft to them", async () => {
      serve();
      const view = show(inbox({ compose: { to: ["atlas"] } }));

      const next = inbox({ compose: { to: ["scout"] } });
      at(next);
      await view.rerender({ place: next });

      expect(chosen("scout")).toBe(true);
      expect(chosen("atlas")).toBe(false);
    });

    it("leaves a draft alone when the place is rebuilt with the same recipients", async () => {
      serve();
      const view = show(inbox({ compose: { to: ["atlas"] } }));
      await userEvent.click(screen.getByRole("button", { name: /^atlas/ }));
      await userEvent.click(screen.getByRole("button", { name: /^scout/ }));
      expect(chosen("scout")).toBe(true);
      expect(chosen("atlas")).toBe(false);

      await view.rerender({ place: inbox({ compose: { to: ["atlas"] } }) });

      expect(chosen("scout")).toBe(true);
      expect(chosen("atlas")).toBe(false);
    });
  });

  describe("a New message after the Inbox was left", () => {
    it("is addressed to the agents it is opened for, not those of the draft left behind", () => {
      serve();
      const first = show(inbox({ compose: { to: ["atlas"] } }));
      expect(chosen("atlas")).toBe(true);
      first.unmount();
      expect(mail.newMessage).toBeNull();

      show(inbox({ compose: { to: ["scout"] } }));
      expect(chosen("scout")).toBe(true);
      expect(chosen("atlas")).toBe(false);
    });

    it("starts empty, whatever was typed before leaving", async () => {
      serve();
      const first = show(inbox({ compose: { to: ["atlas"] } }));
      await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Half a thought");
      first.unmount();

      show(inbox({ compose: { to: ["atlas"] } }));
      expect(screen.getByRole("textbox", { name: /^Message/ })).toHaveProperty("value", "");
    });
  });

  describe("focus", () => {
    it("returns to the list when a New message is cancelled", async () => {
      serve({ folders: { inbox: INBOX_PAGE } });
      const nav = spyNavigation();
      show(inbox({ compose: { to: ["atlas"] } }));
      await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(nav.closeItem).toHaveBeenCalledTimes(1);
      await waitFor(() => {
        expect(document.activeElement).toBe(listColumn());
      });
    });

    it("returns to the list from a New message's Back to list", async () => {
      serve({ folders: { inbox: INBOX_PAGE } });
      const nav = spyNavigation();
      show(inbox({ compose: { to: ["atlas"] } }));
      await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
      expect(nav.closeItem).toHaveBeenCalledTimes(1);
      await waitFor(() => {
        expect(document.activeElement).toBe(listColumn());
      });
    });

    it("returns to the thread's row from Back to list", async () => {
      const nav = await openDeploy();
      await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
      expect(nav.closeItem).toHaveBeenCalledTimes(1);
      await waitFor(() => {
        expect(document.activeElement).toBe(screen.getByRole("link", { name: /Deploy tomorrow/ }));
      });
    });

    it("lands on the new thread's heading after a send", async () => {
      serve({ threads: { [SENT_THREAD]: threadOf(SENT_THREAD) } });
      const nav = spyNavigation();
      const view = show(inbox({ compose: { to: ["atlas"] } }));
      followNavigation(nav, view);
      await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Hello there");
      await userEvent.click(screen.getByRole("button", { name: "Send" }));

      const heading = await screen.findByRole("heading", {
        level: 2,
        name: `Subject of ${SENT_THREAD}`,
      });
      await waitFor(() => {
        expect(document.activeElement).toBe(heading);
      });
      expect(nav.replace).toHaveBeenCalledWith(inbox({ folder: "sent", thread: SENT_THREAD }), {});
    });

    it("waits in the named conversation while the sent thread loads, then moves to its heading", async () => {
      const held = gate();
      serve({ threads: { [SENT_THREAD]: threadOf(SENT_THREAD) }, threadGate: held.promise });
      const nav = spyNavigation();
      const view = show(inbox({ compose: { to: ["atlas"] } }));
      followNavigation(nav, view);
      await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Hello there");
      await userEvent.click(screen.getByRole("button", { name: "Send" }));

      const conversation = await screen.findByRole("region", { name: "Conversation" });
      await waitFor(() => {
        expect(document.activeElement).toBe(conversation);
      });
      held.release();
      const heading = await screen.findByRole("heading", { level: 2 });
      await waitFor(() => {
        expect(document.activeElement).toBe(heading);
      });
    });

    it("moves into the thread's heading when it opens while the list is hidden", async () => {
      serve({
        folders: { inbox: INBOX_PAGE },
        threads: { [DEPLOY.thread_id]: threadOf(DEPLOY.thread_id) },
      });
      const nav = spyNavigation();
      const view = show(inbox());
      followNavigation(nav, view);
      const row = await screen.findByRole("link", { name: /Deploy tomorrow/ });
      listColumn().style.display = "none";

      await userEvent.click(row);
      const heading = await screen.findByRole("heading", {
        level: 2,
        name: `Subject of ${DEPLOY.thread_id}`,
      });
      await waitFor(() => {
        expect(document.activeElement).toBe(heading);
      });
    });

    it("leaves focus on the row when the list stays beside the thread", async () => {
      serve({
        folders: { inbox: INBOX_PAGE },
        threads: { [DEPLOY.thread_id]: threadOf(DEPLOY.thread_id) },
      });
      const nav = spyNavigation();
      const view = show(inbox());
      followNavigation(nav, view);
      const row = await screen.findByRole("link", { name: /Deploy tomorrow/ });

      await userEvent.click(row);
      await screen.findByRole("heading", { level: 2, name: `Subject of ${DEPLOY.thread_id}` });
      await settle();
      expect(document.activeElement).toBe(row);
    });

    it("moves into a New message's heading when it opens while the list is hidden", async () => {
      serve({ folders: { inbox: INBOX_PAGE } });
      const nav = spyNavigation();
      const view = show(inbox());
      followNavigation(nav, view);
      listColumn().style.display = "none";

      await userEvent.click(screen.getByRole("button", { name: "New message" }));
      const heading = await screen.findByRole("heading", { level: 2, name: "New message" });
      await waitFor(() => {
        expect(document.activeElement).toBe(heading);
      });
    });

    it("does not take focus back once the user has moved it while the thread loaded", async () => {
      const held = gate();
      serve({
        folders: { inbox: INBOX_PAGE },
        threads: { [DEPLOY.thread_id]: threadOf(DEPLOY.thread_id) },
        threadGate: held.promise,
      });
      const nav = spyNavigation();
      const view = show(inbox());
      followNavigation(nav, view);
      const row = await screen.findByRole("link", { name: /Deploy tomorrow/ });
      listColumn().style.display = "none";
      await userEvent.click(row);
      await screen.findByRole("region", { name: "Conversation" });

      const elsewhere = screen.getByRole("button", { name: "New message" });
      elsewhere.focus();
      held.release();
      await screen.findByRole("heading", { level: 2, name: `Subject of ${DEPLOY.thread_id}` });
      await settle();
      expect(document.activeElement).toBe(elsewhere);
    });

    it("hands Show older's focus to the first new row when its last page takes the button away", async () => {
      const server = serve({ folders: { inbox: { threads: [DEPLOY], next: "1:x" } } });
      show(inbox());
      await screen.findByRole("link", { name: /Deploy tomorrow/ });

      server.folders.inbox = { threads: [DIGEST], next: null };
      await userEvent.click(screen.getByRole("button", { name: "Show older" }));
      const digest = await screen.findByRole("link", { name: /\(no subject\)/ });
      await waitFor(() => {
        expect(document.activeElement).toBe(digest);
      });
      expect(screen.queryByRole("button", { name: "Show older" })).toBeNull();
    });

    it("keeps focus on Show older while there are more pages", async () => {
      const server = serve({ folders: { inbox: { threads: [DEPLOY], next: "1:x" } } });
      show(inbox());
      await screen.findByRole("link", { name: /Deploy tomorrow/ });

      server.folders.inbox = { threads: [DIGEST], next: "2:y" };
      const older = screen.getByRole("button", { name: "Show older" });
      await userEvent.click(older);
      await screen.findByRole("link", { name: /\(no subject\)/ });
      await settle();
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Show older" }));
      expect(document.activeElement).toBe(older);
    });
  });

  describe("a file panel that shows one of the thread's attachments", () => {
    const owned: Panel = { kind: "file", agent: "atlas", path: ATTACHMENT_PATH };
    const other: Panel = { kind: "file", agent: "atlas", path: "notes/plan.md" };

    it("closes when another thread opens", async () => {
      const nav = await openDeploy(owned);
      await userEvent.click(screen.getByRole("link", { name: /\(no subject\)/ }));
      expect(nav.replace).toHaveBeenCalledWith(inbox({ thread: DIGEST.thread_id }), {});
    });

    it("closes when the folder changes", async () => {
      const nav = await openDeploy(owned);
      await userEvent.click(screen.getByRole("link", { name: "Sent" }));
      expect(nav.replace).toHaveBeenCalledWith(inbox({ folder: "sent" }), {});
    });

    it("closes before the thread does on Back to list", async () => {
      const nav = await openDeploy(owned);
      await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
      expect(nav.closePanel).toHaveBeenCalledTimes(1);
      expect(nav.closeItem).toHaveBeenCalledTimes(1);
      expect(nav.closePanel.mock.invocationCallOrder[0]).toBeLessThan(
        nav.closeItem.mock.invocationCallOrder[0] ?? 0,
      );
    });

    it("keeps the thread open when closing the panel is declined", async () => {
      const nav = await openDeploy(owned);
      nav.closePanel.mockResolvedValue(false);
      await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
      expect(nav.closeItem).not.toHaveBeenCalled();
    });

    it("leaves a panel the thread didn't open alone", async () => {
      const nav = await openDeploy(other);
      await userEvent.click(screen.getByRole("link", { name: /\(no subject\)/ }));
      expect(nav.replace).toHaveBeenCalledWith(inbox({ thread: DIGEST.thread_id }), {
        panel: other,
      });
      await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
      expect(nav.closePanel).not.toHaveBeenCalled();
      expect(nav.closeItem).toHaveBeenCalledTimes(1);
    });
  });
});
