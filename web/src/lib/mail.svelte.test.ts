import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_UPLOAD_BYTES } from "./api";
import type { MailMessage, MailThread, MailThreadPage, MailThreadSummary } from "./hub-types";
import { mailboxOf, MailStore } from "./mail.svelte";
import { INBOX, locationAt } from "./routes";
import { toast } from "./toast.svelte";
import { waitFor } from "../test/wait";

function summary(id: string, overrides: Partial<MailThreadSummary> = {}): MailThreadSummary {
  return {
    thread_id: id,
    subject: id,
    participants: ["atlas"],
    message_count: 1,
    latest_at: "2026-03-14T12:00:00Z",
    snippet: "",
    unread: false,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
    ...overrides,
  };
}

function agentMessage(ref: string, overrides: Partial<MailMessage> = {}): MailMessage {
  return {
    ref,
    from: { kind: "agent", agent: ref.split("/")[0] ?? "atlas" },
    to: [],
    at: "2026-03-14T12:00:00Z",
    title: ref,
    body: "Hello",
    attachments: [],
    in_reply_to: null,
    read: true,
    archived: false,
    seen_by: [],
    ...overrides,
  };
}

function userMessage(ref: string, overrides: Partial<MailMessage> = {}): MailMessage {
  return {
    ...agentMessage(ref),
    from: { kind: "user" },
    to: ["atlas"],
    seen_by: [{ agent: "atlas", seen: false }],
    ...overrides,
  };
}

function thread(id: string, messages: MailMessage[]): MailThread {
  return {
    thread_id: id,
    subject: id,
    participants: ["atlas"],
    messages,
    recipients_for_reply: ["atlas"],
  };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function fail(status: number, error = "nope"): Response {
  return json({ error }, status);
}

function page(threads: MailThreadSummary[], next: string | null = null): Response {
  return json({ threads, next } satisfies MailThreadPage);
}

/** A promise the test settles when it chooses. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** A file whose size is `size`, without holding that many bytes. */
function fileOf(name: string, size = 10): File {
  const file = new File(["x"], name, { type: "text/plain" });
  Object.defineProperty(file, "size", { value: size });
  return file;
}

// The fake hub: what it answers, and every request it was asked, as `METHOD path?query`.
let requests: string[];
let bodies: Record<string, unknown>;
let listAnswer: (url: URL) => Response | Promise<Response>;
let threadAnswer: (ref: string) => Response | Promise<Response>;
let actionAnswer: (action: string, id: string) => Response | Promise<Response>;
let sendAnswer: () => Response | Promise<Response>;
let replyAnswer: (id: string) => Response | Promise<Response>;
let uploadAnswer: (filename: string) => Response | Promise<Response>;
let uploads: number;
/** The abort signal of each upload request, in the order they were made. */
let uploadSignals: (AbortSignal | null | undefined)[];

/** An answer that rejects the way fetch does when `signal` aborts before it arrives. */
function abortable(
  signal: AbortSignal | null | undefined,
  answer: Response | Promise<Response>,
): Promise<Response> {
  if (!signal) return Promise.resolve(answer);
  return new Promise<Response>((resolve, reject) => {
    const abort = (): void => {
      reject(new DOMException("The operation was aborted.", "AbortError"));
    };
    if (signal.aborted) abort();
    else signal.addEventListener("abort", abort, { once: true });
    void Promise.resolve(answer).then(resolve, reject);
  });
}

beforeEach(() => {
  requests = [];
  bodies = {};
  uploads = 0;
  uploadSignals = [];
  listAnswer = () => page([]);
  threadAnswer = () => fail(404);
  actionAnswer = () => new Response(null, { status: 204 });
  sendAnswer = () => json({ thread_id: "atlas/agent/new" });
  replyAnswer = () => json({ message: userMessage("atlas/agent/reply", { body: "Thanks" }) });
  uploadAnswer = (filename) => {
    uploads += 1;
    return json({ id: `upload_${String(uploads)}`, filename, size: 1 });
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = new URL(input, "http://hub.test");
      const method = init?.method ?? "GET";
      const line = `${method} ${url.pathname}${url.search}`;
      requests.push(line);
      if (typeof init?.body === "string") bodies[line] = JSON.parse(init.body);
      if (url.pathname === "/api/hub/uploads") {
        uploadSignals.push(init?.signal);
        return abortable(init?.signal, uploadAnswer(url.searchParams.get("filename") ?? ""));
      }
      if (url.pathname === "/api/hub/mail/send") return sendAnswer();
      if (url.pathname === "/api/hub/mail/threads") return listAnswer(url);
      const match = /^\/api\/hub\/mail\/threads\/([^/]+)(?:\/(\w+))?$/.exec(url.pathname);
      if (match) {
        const [, encoded = "", action] = match;
        const id = decodeURIComponent(encoded);
        if (action === undefined) return threadAnswer(id);
        if (action === "reply") return replyAnswer(id);
        return actionAnswer(action, id);
      }
      return fail(500, `unexpected ${line}`);
    }),
  );
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const ID = "atlas/user/a";
const ENCODED = encodeURIComponent(ID);

describe("the list", () => {
  it("fetches a folder, for everyone or one agent", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary("atlas/user/a"), summary("scout/user/b")]);

    await store.show({ folder: "inbox", agent: null });
    await store.show({ folder: "archived", agent: "scout" });

    expect(requests).toEqual([
      "GET /api/hub/mail/threads?folder=inbox&limit=50",
      "GET /api/hub/mail/threads?folder=archived&agent=scout&limit=50",
    ]);
    expect(store.loaded).toBe(true);
    expect(store.threads.map((t) => t.thread_id)).toEqual(["atlas/user/a", "scout/user/b"]);
  });

  it("keeps the threads on screen while the same list is fetched again, and drops them for another", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)]);
    await store.show({ folder: "inbox", agent: null });

    const held = deferred<Response>();
    listAnswer = () => held.promise;
    const again = store.show({ folder: "inbox", agent: null });
    expect(store.threads).toHaveLength(1);
    expect(store.loaded).toBe(true);
    held.resolve(page([]));
    await again;

    listAnswer = () => page([summary(ID)]);
    await store.show({ folder: "inbox", agent: null });
    const other = deferred<Response>();
    listAnswer = () => other.promise;
    const switching = store.show({ folder: "sent", agent: null });
    expect(store.threads).toEqual([]);
    expect(store.loaded).toBe(false);
    other.resolve(page([]));
    await switching;
  });

  it("pages older threads in with Show older, without repeating one already held", async () => {
    const store = new MailStore();
    listAnswer = (url) =>
      url.searchParams.get("before") === null
        ? page([summary("atlas/user/a"), summary("atlas/user/b")], "cursor-1")
        : page([summary("atlas/user/b"), summary("atlas/user/c")]);
    await store.show({ folder: "inbox", agent: "atlas" });
    expect(store.next).toBe("cursor-1");

    await store.loadMore();

    expect(requests.at(-1)).toBe(
      "GET /api/hub/mail/threads?folder=inbox&agent=atlas&before=cursor-1&limit=50",
    );
    expect(store.threads.map((t) => t.thread_id)).toEqual([
      "atlas/user/a",
      "atlas/user/b",
      "atlas/user/c",
    ]);
    expect(store.next).toBeNull();
    expect(store.loadingMore).toBe(false);
  });

  it("asks for as many threads as it holds when it fetches the list again", async () => {
    const store = new MailStore();
    const many = Array.from({ length: 80 }, (_, n) => summary(`atlas/user/${String(n)}`));
    listAnswer = (url) =>
      url.searchParams.get("before") === null
        ? page(many.slice(0, 50), "cursor-1")
        : page(many.slice(50));
    await store.show({ folder: "inbox", agent: null });
    await store.loadMore();
    listAnswer = () => page(many);

    await store.reload();

    expect(requests.at(-1)).toBe("GET /api/hub/mail/threads?folder=inbox&limit=80");
    expect(store.threads).toHaveLength(80);
  });

  it("says in plain words when the list can't load, and Try again loads it", async () => {
    const store = new MailStore();
    listAnswer = () => fail(500);
    await store.show({ folder: "sent", agent: null });
    expect(store.loaded).toBe(false);
    expect(store.loadError).toMatch(/^Couldn't load your sent messages\. Residuum ran into/);

    listAnswer = () => page([summary(ID)]);
    await store.reload();
    expect(store.loadError).toBeNull();
    expect(store.threads).toHaveLength(1);
  });

  it("says when an older page can't load, keeping what is shown, and tries again", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)], "cursor-1");
    await store.show({ folder: "inbox", agent: null });
    listAnswer = () => Promise.reject(new TypeError("offline"));

    await store.loadMore();

    expect(store.moreError).toMatch(
      /^Couldn't load older conversations\. Residuum isn't reachable/,
    );
    expect(store.threads).toHaveLength(1);
    expect(store.next).toBe("cursor-1");

    listAnswer = () => page([summary("atlas/user/z")]);
    await store.loadMore();
    expect(store.moreError).toBeNull();
    expect(store.threads).toHaveLength(2);
  });

  describe("a fetch of the list that was out while the user changed something", () => {
    const all = { folder: "inbox", agent: null } as const;

    it("doesn't bring back a thread archived meanwhile", async () => {
      const store = new MailStore();
      listAnswer = () => page([summary(ID), summary("scout/user/b")]);
      await store.show(all);
      const stale = deferred<Response>();
      listAnswer = () => stale.promise;
      const reloading = store.reload();

      expect(await store.archive(ID)).toBe(true);
      expect(store.threads.map((t) => t.thread_id)).toEqual(["scout/user/b"]);
      listAnswer = () => page([summary("scout/user/b")]);
      stale.resolve(page([summary(ID), summary("scout/user/b")]));
      await reloading;

      expect(store.threads.map((t) => t.thread_id)).toEqual(["scout/user/b"]);
      expect(requests.filter((r) => r.startsWith("GET /api/hub/mail/threads?"))).toHaveLength(3);
    });

    it("doesn't take a thread restored meanwhile out of the archive again", async () => {
      const store = new MailStore();
      const archived = summary(ID, { archived: true, in_inbox: false });
      listAnswer = () => page([archived, summary("scout/user/b", { archived: true })]);
      await store.show({ folder: "archived", agent: null });
      const stale = deferred<Response>();
      listAnswer = () => stale.promise;
      const reloading = store.reload();

      await store.restore(ID);
      listAnswer = () => page([summary("scout/user/b", { archived: true })]);
      stale.resolve(page([archived, summary("scout/user/b", { archived: true })]));
      await reloading;

      expect(store.threads.map((t) => t.thread_id)).toEqual(["scout/user/b"]);
    });

    it("doesn't make a thread read meanwhile unread again", async () => {
      const store = new MailStore();
      listAnswer = () => page([summary(ID, { unread: true })]);
      await store.show(all);
      const stale = deferred<Response>();
      listAnswer = () => stale.promise;
      const reloading = store.reload();

      expect(await store.markRead(ID)).toBe(true);
      listAnswer = () => page([summary(ID, { unread: false })]);
      stale.resolve(page([summary(ID, { unread: true })]));
      await reloading;

      expect(store.summaryOf(ID)?.unread).toBe(false);
      expect(requests.filter((r) => r.startsWith("GET /api/hub/mail/threads?"))).toHaveLength(3);
    });

    it("lets Show older wait for it and page from the cursor it brings", async () => {
      const store = new MailStore();
      listAnswer = () => page([summary("atlas/user/a"), summary("atlas/user/b")], "cursor-1");
      await store.show(all);
      const stale = deferred<Response>();
      listAnswer = (url) =>
        url.searchParams.get("before") === null ? stale.promise : page([summary("atlas/user/c")]);
      const reloading = store.reload();

      const more = store.loadMore();
      expect(store.loadingMore).toBe(true);
      expect(requests.filter((r) => r.includes("before="))).toEqual([]);

      stale.resolve(page([summary("atlas/user/a"), summary("atlas/user/b")], "cursor-2"));
      await reloading;
      await more;

      expect(requests.at(-1)).toBe(
        "GET /api/hub/mail/threads?folder=inbox&before=cursor-2&limit=50",
      );
      expect(store.threads.map((t) => t.thread_id)).toEqual([
        "atlas/user/a",
        "atlas/user/b",
        "atlas/user/c",
      ]);
      expect(store.next).toBeNull();
      expect(store.loadingMore).toBe(false);
    });

    it("lets Show older stop when the fetch finds the list ended", async () => {
      const store = new MailStore();
      listAnswer = () => page([summary(ID)], "cursor-1");
      await store.show(all);
      const stale = deferred<Response>();
      listAnswer = () => stale.promise;
      const reloading = store.reload();
      const more = store.loadMore();

      stale.resolve(page([summary(ID)]));
      await reloading;
      await more;

      expect(requests.filter((r) => r.includes("before="))).toEqual([]);
      expect(store.loadingMore).toBe(false);
    });
  });

  it("drops an answer for a list that is no longer shown", async () => {
    const store = new MailStore();
    const slow = deferred<Response>();
    listAnswer = () => slow.promise;
    const first = store.show({ folder: "inbox", agent: null });
    listAnswer = () => page([summary("scout/user/b")]);
    await store.show({ folder: "inbox", agent: "scout" });

    slow.resolve(page([summary("atlas/user/a")]));
    await first;

    expect(store.threads.map((t) => t.thread_id)).toEqual(["scout/user/b"]);
  });
});

describe("following the counts", () => {
  it("fetches the list and the open thread again when a count changes, not before or after leaving", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)]);
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.show({ folder: "inbox", agent: null });
    await store.open(ID);
    store.followCounts("atlas=0");
    expect(requests).toHaveLength(2);

    store.followCounts("atlas=0");
    expect(requests).toHaveLength(2);

    const reply = agentMessage("atlas/user/b", { read: true });
    threadAnswer = () => json(thread(ID, [agentMessage(ID), reply]));
    store.followCounts("atlas=1");
    await waitFor(() => {
      expect(store.thread?.messages).toHaveLength(2);
    });
    expect(requests.slice(2)).toEqual([
      "GET /api/hub/mail/threads?folder=inbox&limit=50",
      `GET /api/hub/mail/threads/${ENCODED}`,
    ]);

    store.leave();
    store.followCounts("atlas=2");
    expect(requests).toHaveLength(4);
  });

  it("marks a new unread reply read when it arrives in the open thread", async () => {
    const store = new MailStore();
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    store.followCounts("atlas=0");

    threadAnswer = () =>
      json(thread(ID, [agentMessage(ID), agentMessage("atlas/user/b", { read: false })]));
    store.followCounts("atlas=1");

    await waitFor(() => {
      expect(requests).toContain(`POST /api/hub/mail/threads/${ENCODED}/read`);
    });
    expect(store.thread?.messages.every((m) => m.read)).toBe(true);
  });

  it("keeps the open thread on screen when fetching it again fails", async () => {
    const store = new MailStore();
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    store.followCounts("atlas=0");
    threadAnswer = () => fail(503);

    store.followCounts("atlas=1");
    await waitFor(() => {
      expect(requests.filter((r) => r === `GET /api/hub/mail/threads/${ENCODED}`)).toHaveLength(2);
    });

    expect(store.threadStatus).toBe("ready");
    expect(store.thread?.thread_id).toBe(ID);
  });
});

describe("the open thread", () => {
  it("opens a thread by any message's ref and names it by its own id", async () => {
    const store = new MailStore();
    const member = "scout/user/reply";
    threadAnswer = () => json(thread(ID, [agentMessage(ID), agentMessage(member)]));

    const outcome = await store.open(member);

    expect(outcome).toEqual({ status: "shown", threadId: ID });
    expect(requests).toEqual([`GET /api/hub/mail/threads/${encodeURIComponent(member)}`]);
    expect(store.threadStatus).toBe("ready");
    expect(store.openRef).toBe(member);
    expect(store.openReply?.threadId).toBe(ID);
  });

  it("doesn't fetch the open thread again when the address switches to its own id", async () => {
    const store = new MailStore();
    threadAnswer = () => json(thread(ID, [agentMessage(ID), agentMessage("scout/user/reply")]));
    await store.open("scout/user/reply");

    expect(await store.open(ID)).toEqual({ status: "shown", threadId: ID });
    expect(requests).toHaveLength(1);
  });

  it("says when the thread is gone", async () => {
    const store = new MailStore();
    threadAnswer = () => fail(404);

    expect(await store.open(ID)).toEqual({ status: "missing" });
    expect(store.threadStatus).toBe("missing");
    expect(store.thread).toBeNull();
  });

  it("says in plain words when the thread can't load, and Try again opens it", async () => {
    const store = new MailStore();
    threadAnswer = () => fail(502);
    expect(await store.open(ID)).toEqual({ status: "failed" });
    expect(store.threadStatus).toBe("error");
    expect(store.threadError).toMatch(/^Couldn't open that conversation\./);

    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    expect(await store.retryOpen()).toEqual({ status: "shown", threadId: ID });
    expect(store.threadError).toBeNull();
  });

  it("shows only the thread opened last", async () => {
    const store = new MailStore();
    const slow = deferred<Response>();
    threadAnswer = () => slow.promise;
    const first = store.open(ID);
    threadAnswer = () => json(thread("scout/user/b", [agentMessage("scout/user/b")]));
    await store.open("scout/user/b");

    slow.resolve(json(thread(ID, [agentMessage(ID)])));
    expect(await first).toEqual({ status: "superseded" });
    expect(store.thread?.thread_id).toBe("scout/user/b");
  });

  it("closing drops the thread and an answer still on its way", async () => {
    const store = new MailStore();
    const slow = deferred<Response>();
    threadAnswer = () => slow.promise;
    const opening = store.open(ID);
    store.close();
    slow.resolve(json(thread(ID, [agentMessage(ID)])));

    expect(await opening).toEqual({ status: "superseded" });
    expect(store.thread).toBeNull();
    expect(store.threadStatus).toBe("idle");
    expect(store.openRef).toBeNull();
  });

  it("marks an unread thread read at once in the list and the thread", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID, { unread: true })]);
    await store.show({ folder: "inbox", agent: null });
    const held = deferred<Response>();
    actionAnswer = () => held.promise;
    threadAnswer = () => json(thread(ID, [agentMessage(ID, { read: false }), userMessage("x")]));

    await store.open(ID);

    expect(requests).toContain(`POST /api/hub/mail/threads/${ENCODED}/read`);
    expect(store.summaryOf(ID)?.unread).toBe(false);
    expect(store.thread?.messages[0]?.read).toBe(true);
    held.resolve(new Response(null, { status: 204 }));
    await waitFor(() => {
      expect(store.problems).toEqual({});
    });
  });

  it("doesn't mark a thread that is already read", async () => {
    const store = new MailStore();
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    expect(requests).toEqual([`GET /api/hub/mail/threads/${ENCODED}`]);
  });

  it("puts a read the hub refused back as unread, says why, and tries it again", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID, { unread: true })]);
    await store.show({ folder: "inbox", agent: null });
    actionAnswer = () => fail(503);
    threadAnswer = () => json(thread(ID, [agentMessage(ID, { read: false })]));

    await store.open(ID);
    await waitFor(() => {
      expect(store.problems[ID]?.action).toBe("read");
    });

    expect(store.problems[ID]?.message).toMatch(/^Couldn't mark it read\./);
    expect(store.summaryOf(ID)?.unread).toBe(true);
    expect(store.thread?.messages[0]?.read).toBe(false);

    actionAnswer = () => new Response(null, { status: 204 });
    expect(await store.retry(ID)).toBe(true);
    expect(store.problems[ID]).toBeUndefined();
    expect(store.summaryOf(ID)?.unread).toBe(false);
    expect(store.thread?.messages[0]?.read).toBe(true);
  });
});

describe("archiving and restoring", () => {
  it("takes an archived thread out of the Inbox list and marks the open thread archived", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID), summary("scout/user/b")]);
    threadAnswer = () => json(thread(ID, [agentMessage(ID), userMessage("atlas/agent/q")]));
    await store.show({ folder: "inbox", agent: null });
    await store.open(ID);
    expect(store.thread && mailboxOf(store.thread)).toBe("inbox");

    expect(await store.archive(ID)).toBe(true);

    expect(requests.at(-1)).toBe(`POST /api/hub/mail/threads/${ENCODED}/archive`);
    expect(store.threads.map((t) => t.thread_id)).toEqual(["scout/user/b"]);
    expect(store.thread && mailboxOf(store.thread)).toBe("archived");
    expect(store.thread?.messages[1]?.archived).toBe(false);
  });

  it("brings the thread back into the Inbox list on Undo", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)]);
    await store.show({ folder: "inbox", agent: null });
    await store.archive(ID);
    expect(store.threads).toEqual([]);

    expect(await store.restore(ID)).toBe(true);

    await waitFor(() => {
      expect(store.threads.map((t) => t.thread_id)).toEqual([ID]);
    });
    expect(requests.slice(-2)).toEqual([
      `POST /api/hub/mail/threads/${ENCODED}/restore`,
      "GET /api/hub/mail/threads?folder=inbox&limit=50",
    ]);
  });

  it("takes a restored thread out of the archive list", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID, { archived: true, in_inbox: false })]);
    await store.show({ folder: "archived", agent: null });

    await store.restore(ID);

    expect(store.threads).toEqual([]);
  });

  it("updates a thread in the Sent list in place", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID, { in_sent: true })]);
    await store.show({ folder: "sent", agent: null });

    await store.archive(ID);

    expect(store.threads).toEqual([
      summary(ID, { in_sent: true, archived: true, in_inbox: false }),
    ]);
  });

  it("shows an archived thread as read in a list that still holds it", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID, { in_sent: true, unread: true })]);
    await store.show({ folder: "sent", agent: null });

    await store.archive(ID);

    expect(store.summaryOf(ID)).toMatchObject({ archived: true, unread: false });
  });

  it("keeps one action at a time on a thread", async () => {
    const store = new MailStore();
    const held = deferred<Response>();
    actionAnswer = () => held.promise;
    const archiving = store.archive(ID);
    expect(store.pending[ID]).toBe("archive");

    expect(await store.restore(ID)).toBe(false);
    expect(await store.archive(ID)).toBe(false);

    held.resolve(new Response(null, { status: 204 }));
    expect(await archiving).toBe(true);
    expect(store.pending).toEqual({});
    expect(requests).toEqual([`POST /api/hub/mail/threads/${ENCODED}/archive`]);
  });

  it("leaves a thread whose archive failed in place, says why, and Try again archives it", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)]);
    await store.show({ folder: "inbox", agent: null });
    actionAnswer = () => fail(503);

    expect(await store.archive(ID)).toBe(false);

    expect(store.threads).toHaveLength(1);
    expect(store.problems[ID]).toEqual({
      action: "archive",
      message: expect.stringMatching(/^Couldn't archive it\. Residuum ran into/) as string,
    });

    actionAnswer = () => new Response(null, { status: 204 });
    expect(await store.retry(ID)).toBe(true);
    expect(store.problems[ID]).toBeUndefined();
    expect(store.threads).toEqual([]);
  });

  it("says a thread that has gone is no longer there", async () => {
    const store = new MailStore();
    actionAnswer = () => fail(404);
    await store.restore(ID);
    expect(store.problems[ID]?.message).toBe(
      "Couldn't move it to the inbox. That conversation isn't there any more.",
    );
  });

  it("reads where a thread's agent messages are", () => {
    expect(mailboxOf(thread(ID, [userMessage("atlas/agent/q")]))).toBeNull();
    expect(
      mailboxOf(thread(ID, [agentMessage(ID, { archived: true }), agentMessage("atlas/user/b")])),
    ).toBe("inbox");
    expect(mailboxOf(thread(ID, [agentMessage(ID, { archived: true })]))).toBe("archived");
  });
});

describe("attachments", () => {
  it("uploads a picked file at once, and it is pending until the hub stages it", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);
    const held = deferred<Response>();
    uploadAnswer = () => held.promise;

    draft.attach([fileOf("notes.md")]);

    const [attachment] = draft.attachments;
    expect(attachment?.status).toBe("uploading");
    expect(draft.uploading).toBe(true);
    expect(requests).toEqual(["POST /api/hub/uploads?filename=notes.md"]);
    held.resolve(json({ id: "upload_9", filename: "notes.md", size: 10 }));
    await attachment?.done;
    expect(attachment?.status).toBe("uploaded");
    expect(attachment?.uploadId).toBe("upload_9");
  });

  it("refuses a file over the limit before uploading, naming it and the limit", () => {
    const store = new MailStore();
    const draft = store.openNewMessage();

    draft.attach([fileOf("huge.mov", MAX_UPLOAD_BYTES + 1), fileOf("ok.txt", MAX_UPLOAD_BYTES)]);

    expect(draft.attachNotice).toBe(
      "huge.mov is larger than the 50 MB upload limit, so it wasn't attached.",
    );
    expect(draft.attachments.map((a) => a.filename)).toEqual(["ok.txt"]);
    expect(requests).toEqual(["POST /api/hub/uploads?filename=ok.txt"]);

    draft.attach([fileOf("a.iso", MAX_UPLOAD_BYTES + 1), fileOf("b.iso", MAX_UPLOAD_BYTES + 1)]);
    expect(draft.attachNotice).toBe(
      "a.iso and b.iso are larger than the 50 MB upload limit, so they weren't attached.",
    );
    draft.dismissAttachNotice();
    expect(draft.attachNotice).toBeNull();
  });

  it("shows a failed upload, and Try again uploads it", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    uploadAnswer = () => fail(413);
    draft.attach([fileOf("report.pdf")]);
    const [attachment] = draft.attachments;
    await attachment?.done;

    expect(attachment?.status).toBe("failed");
    expect(attachment?.error).toBe(
      "Couldn't upload report.pdf. That file is larger than the 50 MB upload limit.",
    );

    uploadAnswer = () => json({ id: "upload_2", filename: "report.pdf", size: 10 });
    await draft.retryAttachment(attachment?.key ?? "");
    expect(attachment?.status).toBe("uploaded");
    expect(attachment?.error).toBeNull();
  });

  it("removes an attachment, even one still uploading", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    const held = deferred<Response>();
    uploadAnswer = () => held.promise;
    draft.attach([fileOf("a.txt")]);
    const [attachment] = draft.attachments;

    draft.removeAttachment(attachment?.key ?? "");
    held.resolve(json({ id: "upload_1", filename: "a.txt", size: 1 }));
    await attachment?.done;

    expect(draft.attachments).toEqual([]);
    expect(attachment?.uploadId).toBeNull();
  });

  it("cancels the upload of an attachment removed before it finished", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    const held = deferred<Response>();
    uploadAnswer = () => held.promise;
    draft.attach([fileOf("a.txt"), fileOf("b.txt")]);
    const [removed, kept] = draft.attachments;
    const [removedSignal, keptSignal] = uploadSignals;
    expect(removedSignal?.aborted).toBe(false);

    draft.removeAttachment(removed?.key ?? "");
    await removed?.done;

    expect(removedSignal?.aborted).toBe(true);
    expect(keptSignal?.aborted).toBe(false);
    expect(removed?.error).toBeNull();
    expect(kept?.status).toBe("uploading");
  });

  it("cancels the uploads still running when the draft is thrown away", () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    uploadAnswer = () => deferred<Response>().promise;
    draft.attach([fileOf("a.txt")]);
    const [signal] = uploadSignals;
    expect(draft.attachments).toHaveLength(1);

    store.discardNewMessage();

    expect(signal?.aborted).toBe(true);
  });

  it("gives a retried upload a signal of its own", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    uploadAnswer = () => fail(500);
    draft.attach([fileOf("a.txt")]);
    const [attachment] = draft.attachments;
    await attachment?.done;
    expect(attachment?.status).toBe("failed");

    uploadAnswer = () => deferred<Response>().promise;
    void draft.retryAttachment(attachment?.key ?? "");
    draft.removeAttachment(attachment?.key ?? "");

    expect(uploadSignals).toHaveLength(2);
    expect(uploadSignals[0]).not.toBe(uploadSignals[1]);
    expect(uploadSignals[1]?.aborted).toBe(true);
  });
});

describe("a new message", () => {
  it("addresses a New message with nothing written to whoever it is opened for", () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);

    // Recipients alone are not something the user wrote.
    expect(store.openNewMessage(["scout"])).toBe(draft);
    expect(draft.to).toEqual(["scout"]);
    expect(store.openNewMessage()).toBe(draft);
    expect(draft.to).toEqual([]);

    store.discardNewMessage();
    expect(store.newMessage).toBeNull();
    expect(store.openNewMessage(["scout"]).to).toEqual(["scout"]);
  });

  it("keeps what was written in a New message, and adds the recipients it lacks", () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);
    draft.body = "Hi";

    expect(store.openNewMessage(["scout", "atlas"])).toBe(draft);
    expect(draft.to).toEqual(["atlas", "scout"]);
    expect(draft.body).toBe("Hi");

    expect(store.openNewMessage()).toBe(draft);
    expect(draft.to).toEqual(["atlas", "scout"]);
  });

  it("counts a subject or an attachment as something written", () => {
    const store = new MailStore();
    const subject = store.openNewMessage(["atlas"]);
    subject.subject = "Plan";
    expect(store.openNewMessage(["scout"]).to).toEqual(["atlas", "scout"]);

    store.discardNewMessage();
    const attached = store.openNewMessage(["atlas"]);
    attached.attach([fileOf("a.txt")]);
    expect(store.openNewMessage(["scout"]).to).toEqual(["atlas", "scout"]);
  });

  it("needs a recipient and a body, and allows an empty subject", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage();
    expect(draft.invalid).toBe("Add at least one agent to send it to.");
    draft.to = ["atlas"];
    draft.body = "  \n ";
    expect(draft.invalid).toBe("Write a message before sending it.");

    expect(await store.sendNewMessage()).toBeNull();
    expect(draft.sendError).toBe("Write a message before sending it.");
    expect(requests).toEqual([]);

    draft.body = "Hello";
    expect(draft.invalid).toBeNull();
    expect(await store.sendNewMessage()).toBe("atlas/agent/new");
    expect(bodies["POST /api/hub/mail/send"]).toEqual({
      to: ["atlas"],
      subject: "",
      body: "Hello",
      attachments: [],
    });
  });

  it("waits for uploads, sends their ids, closes the draft and fetches the list again", async () => {
    const store = new MailStore();
    listAnswer = () => page([]);
    await store.show({ folder: "sent", agent: null });
    const draft = store.openNewMessage(["atlas", "scout"]);
    draft.subject = " Plan ";
    draft.body = "See attached";
    const held = deferred<Response>();
    uploadAnswer = () => held.promise;
    draft.attach([fileOf("plan.md")]);

    const sending = store.sendNewMessage();
    expect(draft.sending).toBe(true);
    await Promise.resolve();
    expect(requests).not.toContain("POST /api/hub/mail/send");

    listAnswer = () => page([summary("atlas/agent/new", { in_sent: true })]);
    held.resolve(json({ id: "upload_7", filename: "plan.md", size: 10 }));
    expect(await sending).toBe("atlas/agent/new");

    expect(bodies["POST /api/hub/mail/send"]).toEqual({
      to: ["atlas", "scout"],
      subject: "Plan",
      body: "See attached",
      attachments: ["upload_7"],
    });
    expect(store.newMessage).toBeNull();
    expect(draft.sending).toBe(false);
    await waitFor(() => {
      expect(store.threads.map((t) => t.thread_id)).toEqual(["atlas/agent/new"]);
    });
  });

  it("doesn't send while an attachment failed to upload, and says which", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);
    draft.body = "Hi";
    uploadAnswer = () => fail(500);
    draft.attach([fileOf("log.txt")]);

    expect(await store.sendNewMessage()).toBeNull();

    expect(draft.sendError).toBe("log.txt didn't upload. Try it again or remove it, then send.");
    expect(requests).not.toContain("POST /api/hub/mail/send");
    expect(store.newMessage).toBe(draft);
  });

  it("keeps the draft and says why when the hub refuses it", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["ghost"]);
    draft.body = "Hi";
    sendAnswer = () => fail(400, "There is no agent named 'ghost'.");

    expect(await store.sendNewMessage()).toBeNull();

    expect(draft.sendError).toBe("Couldn't send your message. There is no agent named 'ghost'.");
    expect(store.newMessage).toBe(draft);
    expect(draft.body).toBe("Hi");
    expect(draft.sending).toBe(false);
  });

  it("waits for a file attached while it waits, rather than reporting it as failed", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);
    draft.body = "Two files";
    const first = deferred<Response>();
    const second = deferred<Response>();
    uploadAnswer = (filename) => (filename === "a.txt" ? first.promise : second.promise);
    draft.attach([fileOf("a.txt")]);

    const sending = store.sendNewMessage();
    draft.attach([fileOf("b.txt")]);
    first.resolve(json({ id: "upload_a", filename: "a.txt", size: 1 }));
    await waitFor(() => {
      expect(draft.attachments[0]?.status).toBe("uploaded");
    });
    expect(draft.sendError).toBeNull();
    expect(requests).not.toContain("POST /api/hub/mail/send");

    second.resolve(json({ id: "upload_b", filename: "b.txt", size: 1 }));
    expect(await sending).toBe("atlas/agent/new");
    expect(bodies["POST /api/hub/mail/send"]).toMatchObject({
      attachments: ["upload_a", "upload_b"],
    });
  });

  describe("when the user leaves while it is being sent", () => {
    it("goes on sending what was written, resolves nothing to open, and says it went", async () => {
      const success = vi.spyOn(toast, "success").mockImplementation(() => 0);
      const store = new MailStore();
      const draft = store.openNewMessage(["atlas", "scout"]);
      draft.body = "Still going";
      const held = deferred<Response>();
      uploadAnswer = () => held.promise;
      draft.attach([fileOf("plan.md")]);
      const sending = store.sendNewMessage();

      store.discardNewMessage();
      expect(store.newMessage).toBeNull();
      held.resolve(json({ id: "upload_9", filename: "plan.md", size: 1 }));

      expect(await sending).toBeNull();
      expect(bodies["POST /api/hub/mail/send"]).toEqual({
        to: ["atlas", "scout"],
        subject: "",
        body: "Still going",
        attachments: ["upload_9"],
      });
      expect(success).toHaveBeenCalledWith("Sent to atlas and scout.");
      expect(store.newMessage).toBeNull();
    });

    it("says so when it didn't go, since nothing else shows it", async () => {
      const error = vi.spyOn(toast, "error").mockImplementation(() => 0);
      const store = new MailStore();
      const draft = store.openNewMessage(["atlas"]);
      draft.body = "Lost?";
      sendAnswer = () => fail(400, "There is no agent named 'atlas'.");
      const held = deferred<Response>();
      uploadAnswer = () => held.promise;
      draft.attach([fileOf("a.txt")]);
      const sending = store.sendNewMessage();

      store.discardNewMessage();
      held.resolve(json({ id: "upload_1", filename: "a.txt", size: 1 }));

      expect(await sending).toBeNull();
      expect(error).toHaveBeenCalledWith(
        "Your message to atlas wasn't sent. There is no agent named 'atlas'.",
      );
    });

    it("says so when a file didn't upload", async () => {
      const error = vi.spyOn(toast, "error").mockImplementation(() => 0);
      const store = new MailStore();
      const draft = store.openNewMessage(["atlas"]);
      draft.body = "Lost?";
      const held = deferred<Response>();
      uploadAnswer = () => held.promise;
      draft.attach([fileOf("a.txt")]);
      const sending = store.sendNewMessage();

      store.discardNewMessage();
      held.resolve(fail(500));

      expect(await sending).toBeNull();
      expect(error).toHaveBeenCalledWith(
        "Your message to atlas wasn't sent because a file didn't upload.",
      );
      expect(requests).not.toContain("POST /api/hub/mail/send");
    });

    it("doesn't ask about the draft, which is going, but does when the page itself goes away", async () => {
      const store = new MailStore();
      const draft = store.openNewMessage(["atlas"]);
      draft.body = "Going";
      const held = deferred<Response>();
      uploadAnswer = () => held.promise;
      draft.attach([fileOf("a.txt")]);
      const sending = store.sendNewMessage();
      const list = locationAt(INBOX);

      expect(store.draftLoss(list)).toBeNull();
      expect(store.draftLoss(null)).toBe("An unsent new message");

      held.resolve(json({ id: "upload_1", filename: "a.txt", size: 1 }));
      await sending;
    });
  });
});

describe("replies", () => {
  it("adds the reply to the open thread, empties the draft and fetches the list again", async () => {
    const store = new MailStore();
    listAnswer = () => page([summary(ID)]);
    await store.show({ folder: "inbox", agent: null });
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    const draft = store.openReply;
    if (draft === null) throw new Error("no reply draft");
    draft.body = "Thanks";
    draft.attach([fileOf("a.txt")]);

    expect(await store.sendReply(ID)).toBe(true);

    expect(bodies[`POST /api/hub/mail/threads/${ENCODED}/reply`]).toEqual({
      body: "Thanks",
      attachments: ["upload_1"],
    });
    expect(store.thread?.messages.map((m) => m.ref)).toEqual([ID, "atlas/agent/reply"]);
    expect(draft.body).toBe("");
    expect(draft.attachments).toEqual([]);
    await waitFor(() => {
      expect(requests.at(-1)).toBe("GET /api/hub/mail/threads?folder=inbox&limit=50");
    });
  });

  describe("a fetch of the open thread that was out while the user changed it", () => {
    const refreshing = async (store: MailStore, stale: Promise<Response>): Promise<void> => {
      threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
      await store.open(ID);
      store.followCounts("atlas=0");
      threadAnswer = () => stale;
      store.followCounts("atlas=1");
      await waitFor(() => {
        expect(requests.filter((r) => r === `GET /api/hub/mail/threads/${ENCODED}`)).toHaveLength(
          2,
        );
      });
    };

    it("doesn't drop a reply sent meanwhile", async () => {
      const store = new MailStore();
      const stale = deferred<Response>();
      await refreshing(store, stale.promise);
      const draft = store.openReply;
      if (draft === null) throw new Error("no reply draft");
      draft.body = "Thanks";

      expect(await store.sendReply(ID)).toBe(true);
      expect(store.thread?.messages.map((m) => m.ref)).toEqual([ID, "atlas/agent/reply"]);
      threadAnswer = () =>
        json(thread(ID, [agentMessage(ID), userMessage("atlas/agent/reply", { body: "Thanks" })]));
      stale.resolve(json(thread(ID, [agentMessage(ID)])));

      await waitFor(() => {
        expect(requests.filter((r) => r === `GET /api/hub/mail/threads/${ENCODED}`)).toHaveLength(
          3,
        );
      });
      await waitFor(() => {
        expect(store.thread?.messages.map((m) => m.ref)).toEqual([ID, "atlas/agent/reply"]);
      });
    });

    it("doesn't undo an archive made meanwhile", async () => {
      const store = new MailStore();
      const stale = deferred<Response>();
      await refreshing(store, stale.promise);

      await store.archive(ID);
      expect(store.thread && mailboxOf(store.thread)).toBe("archived");
      threadAnswer = () => json(thread(ID, [agentMessage(ID, { archived: true })]));
      stale.resolve(json(thread(ID, [agentMessage(ID)])));

      await waitFor(() => {
        expect(requests.filter((r) => r === `GET /api/hub/mail/threads/${ENCODED}`)).toHaveLength(
          3,
        );
      });
      await waitFor(() => {
        expect(store.thread && mailboxOf(store.thread)).toBe("archived");
      });
    });

    it("doesn't add a reply twice when a fetch already brought it", async () => {
      const store = new MailStore();
      threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
      await store.open(ID);
      const draft = store.openReply;
      if (draft === null) throw new Error("no reply draft");
      draft.body = "Thanks";
      const sent = userMessage("atlas/agent/reply", { body: "Thanks" });
      const held = deferred<Response>();
      replyAnswer = () => held.promise;
      const sending = store.sendReply(ID);
      await waitFor(() => {
        expect(requests).toContain(`POST /api/hub/mail/threads/${ENCODED}/reply`);
      });

      threadAnswer = () => json(thread(ID, [agentMessage(ID), sent]));
      store.followCounts("atlas=0");
      store.followCounts("atlas=1");
      await waitFor(() => {
        expect(store.thread?.messages).toHaveLength(2);
      });
      held.resolve(json({ message: sent }));

      expect(await sending).toBe(true);
      expect(store.thread?.messages.map((m) => m.ref)).toEqual([ID, "atlas/agent/reply"]);
    });
  });

  it("keeps each thread's reply draft across closing and opening threads", async () => {
    const store = new MailStore();
    threadAnswer = (ref) => json(thread(ref, [agentMessage(ref)]));
    await store.open(ID);
    const draft = store.openReply;
    if (draft === null) throw new Error("no reply draft");
    draft.body = "Half written";

    store.close();
    await store.open("scout/user/b");
    expect(store.openReply?.body).toBe("");
    await store.open(ID);

    expect(store.openReply).toBe(draft);
    expect(store.replyDraft(ID)?.body).toBe("Half written");
    store.discardReply(ID);
    expect(draft.body).toBe("");
  });

  it("needs a body, and keeps the draft when the reply is refused", async () => {
    const store = new MailStore();
    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    const draft = store.openReply;
    if (draft === null) throw new Error("no reply draft");

    expect(await store.sendReply(ID)).toBe(false);
    expect(draft.sendError).toBe("Write a reply before sending it.");

    draft.body = "Hi";
    replyAnswer = () => fail(404);
    expect(await store.sendReply(ID)).toBe(false);
    expect(draft.sendError).toBe(
      "Couldn't send your reply. That conversation isn't there any more.",
    );
    expect(draft.body).toBe("Hi");
    expect(store.thread?.messages).toHaveLength(1);
  });
});

describe("unsaved drafts", () => {
  it("counts what the user wrote or attached as unsaved, not recipients alone", async () => {
    const store = new MailStore();
    const draft = store.openNewMessage(["atlas"]);
    expect(store.dirty).toBe(false);
    draft.subject = "Plan";
    expect(store.dirty).toBe(true);
    draft.subject = "";
    draft.attach([fileOf("a.txt")]);
    expect(store.dirty).toBe(true);
    store.discardNewMessage();
    expect(store.dirty).toBe(false);

    threadAnswer = () => json(thread(ID, [agentMessage(ID)]));
    await store.open(ID);
    const reply = store.openReply;
    if (reply === null) throw new Error("no reply draft");
    reply.body = "Hi";
    expect(store.dirty).toBe(true);
  });

  it("says the New message is lost unless the target keeps it open, and replies only when the page goes", async () => {
    const store = new MailStore();
    const compose = locationAt({ ...INBOX, compose: { to: [] } });
    const list = locationAt(INBOX);
    expect(store.draftLoss(null)).toBeNull();

    store.openNewMessage(["atlas"]).body = "Hi";
    expect(store.draftLoss(compose)).toBeNull();
    expect(store.draftLoss(list)).toBe("An unsent new message");

    threadAnswer = (ref) => json(thread(ref, [agentMessage(ref)]));
    await store.open(ID);
    if (store.openReply) store.openReply.body = "One";
    expect(store.draftLoss(list)).toBe("An unsent new message");
    expect(store.draftLoss(null)).toBe("An unsent new message and an unsent reply");

    store.discardNewMessage();
    await store.open("scout/user/b");
    if (store.openReply) store.openReply.body = "Two";
    expect(store.draftLoss(list)).toBeNull();
    expect(store.draftLoss(null)).toBe("2 unsent replies");
  });
});
