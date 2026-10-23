import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { hub } from "../../lib/hub.svelte";
import type {
  AgentOverview,
  AgentSummary,
  MailFolder,
  MailThread,
  MailThreadPage,
  MailThreadSummary,
} from "../../lib/hub-types";
import { mail } from "../../lib/mail.svelte";
import { overview } from "../../lib/overview.svelte";
import { router } from "../../lib/router.svelte";
import { HOME, INBOX, locationAt, type InboxPlace, type Panel } from "../../lib/routes";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { waitFor } from "../../test/wait";
import Inbox from "./Inbox.svelte";

const ATTACHMENT_PATH = "inbox/user/attachments/mock_1/notes.md";

function summary(id: string, overrides: Partial<MailThreadSummary> = {}): MailThreadSummary {
  return {
    thread_id: id,
    subject: `Subject of ${id}`,
    participants: [id.split("/")[0] ?? "atlas"],
    message_count: 1,
    latest_at: "2026-03-14T11:00:00Z",
    snippet: `Snippet of ${id}`,
    unread: false,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
    ...overrides,
  };
}

function threadOf(id: string): MailThread {
  const agent = id.split("/")[0] ?? "atlas";
  return {
    thread_id: id,
    subject: `Subject of ${id}`,
    participants: [agent],
    messages: [
      {
        ref: id,
        from: { kind: "agent", agent },
        to: [],
        at: "2026-03-14T11:00:00Z",
        title: `Subject of ${id}`,
        body: "Hello",
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
        in_reply_to: null,
        read: true,
        archived: false,
        seen_by: [],
      },
    ],
    recipients_for_reply: [agent],
  };
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
  requests: string[];
}

function serve(initial: Partial<Server> = {}): Server {
  const server: Server = {
    folders: {},
    threads: {},
    failList: false,
    holdList: false,
    requests: [],
    ...initial,
  };
  mockFetch((url, init) => {
    const parsed = new URL(url, "http://localhost");
    const method = init?.method ?? "GET";
    server.requests.push(`${method} ${parsed.pathname}${parsed.search}`);
    if (parsed.pathname === "/api/hub/mail/threads") {
      if (server.holdList) return new Promise<Response>(() => {});
      if (server.failList) return jsonResponse({ error: "boom" }, 500);
      const folder = (parsed.searchParams.get("folder") ?? "inbox") as MailFolder;
      return jsonResponse(server.folders[folder] ?? { threads: [], next: null });
    }
    const match = /^\/api\/hub\/mail\/threads\/([^/]+)(\/read)?$/.exec(parsed.pathname);
    if (match?.[1] !== undefined) {
      if (match[2] !== undefined) return new Response(null, { status: 204 });
      const thread = server.threads[decodeURIComponent(match[1])];
      return thread === undefined
        ? jsonResponse({ error: "There is no such thread." }, 404)
        : jsonResponse(thread);
    }
    if (parsed.pathname === "/api/hub/mail/send") {
      return jsonResponse({ thread_id: "atlas/agent/20260314_hello" });
    }
    return jsonResponse({});
  });
  return server;
}

const listRequests = (server: Server): string[] =>
  server.requests.filter((r) => r.startsWith("GET /api/hub/mail/threads?"));

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

function show(
  place: InboxPlace,
  panel: Panel | null = null,
): ReturnType<typeof render<typeof Inbox>> {
  at(place, panel);
  return render(Inbox, { place });
}

const DEPLOY = summary("atlas/user/mock_1", { unread: true, subject: "Deploy tomorrow" });
const DIGEST = summary("scout/user/mock_2", { subject: "", has_attachments: true });
const INBOX_PAGE: MailThreadPage = { threads: [DEPLOY, DIGEST], next: null };

beforeEach(() => {
  stubWebSocket();
  Element.prototype.scrollIntoView = vi.fn();
  hub.agents = [agentSummary("atlas"), agentSummary("scout")];
  overview.overviews = { atlas: overviewOf("atlas", 2), scout: overviewOf("scout", 1) };
  mail.close();
  mail.list = null;
  mail.threads = [];
  mail.loaded = false;
  mail.loadError = null;
  mail.moreError = null;
  mail.next = null;
  mail.newMessage = null;
  mail.replies = {};
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
      expect(nav.replace).toHaveBeenCalledWith(
        inbox({ folder: "sent", thread: "atlas/agent/20260314_hello" }),
        {},
      );
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

  describe("a file panel that shows one of the thread's attachments", () => {
    const owned: Panel = { kind: "file", agent: "atlas", path: ATTACHMENT_PATH };
    const other: Panel = { kind: "file", agent: "atlas", path: "notes/plan.md" };

    async function openDeploy(panel: Panel): Promise<NavigationSpies> {
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
