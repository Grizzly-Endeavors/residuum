import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HubInboxItem, HubInboxPage } from "./hub-types";
import { InboxStore } from "./inbox.svelte";

function item(agent: string, id: string, overrides: Partial<HubInboxItem> = {}): HubInboxItem {
  return {
    agent,
    id,
    title: `${agent} ${id}`,
    body: "",
    source: "agent",
    at: "2026-03-14T12:00:00Z",
    read: false,
    attachments: [],
    ...overrides,
  };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** The fake hub: what it answers, and every request it was asked, as `METHOD path?query`. */
let requests: string[];
let listAnswer: (url: URL) => Response | Promise<Response>;
let itemAnswer: (action: string, agent: string, id: string) => Response;

function page(items: HubInboxItem[], next: string | null = null): Response {
  return json({ items, next_cursor: next } satisfies HubInboxPage);
}

beforeEach(() => {
  requests = [];
  listAnswer = () => page([]);
  itemAnswer = (_action, agent, id) => json({ item: item(agent, id, { read: true }) });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      const url = new URL(input, "http://hub.test");
      requests.push(`${init?.method ?? "GET"} ${url.pathname}${url.search}`);
      const match = /^\/api\/hub\/inbox\/([^/]+)\/([^/]+)\/(read|archive|restore)$/.exec(
        url.pathname,
      );
      if (match) {
        const [, agent = "", id = "", action = ""] = match;
        return itemAnswer(action, decodeURIComponent(agent), decodeURIComponent(id));
      }
      return listAnswer(url);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the list", () => {
  it("fetches the inbox or the archive, for everyone or one agent", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a"), item("scout", "a")]);

    await store.show({ agent: null, tab: "active" });
    await store.show({ agent: "scout", tab: "archived" });

    expect(requests).toEqual([
      "GET /api/hub/inbox?status=active&limit=50",
      "GET /api/hub/inbox?status=archived&agent=scout&limit=50",
    ]);
    expect(store.loaded).toBe(true);
    expect(store.items.map((i) => `${i.agent}:${i.id}`)).toEqual(["atlas:a", "scout:a"]);
  });

  it("keeps the items on screen while the same list is fetched again, and drops them for another", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")]);
    await store.show({ agent: null, tab: "active" });

    let release = (): void => {};
    listAnswer = () =>
      new Promise((resolve) => {
        release = () => {
          resolve(page([item("atlas", "b")]));
        };
      });
    const again = store.show({ agent: null, tab: "active" });
    expect(store.items).toHaveLength(1);
    release();
    await again;
    expect(store.items.map((i) => i.id)).toEqual(["b"]);

    const other = store.show({ agent: "atlas", tab: "archived" });
    expect(store.items).toEqual([]);
    expect(store.loaded).toBe(false);
    release();
    await other;
  });

  it("drops an answer for a list that is no longer shown", async () => {
    const store = new InboxStore();
    let release = (): void => {};
    listAnswer = (url) =>
      url.searchParams.get("status") === "active"
        ? new Promise((resolve) => {
            release = () => {
              resolve(page([item("atlas", "stale")]));
            };
          })
        : page([item("atlas", "archived")]);

    const first = store.show({ agent: null, tab: "active" });
    await store.show({ agent: null, tab: "archived" });
    release();
    await first;

    expect(store.items.map((i) => i.id)).toEqual(["archived"]);
  });

  it("says why the list couldn't load, and loads it on Try again", async () => {
    const store = new InboxStore();
    listAnswer = () => json({ error: "boom" }, 500);
    await store.show({ agent: null, tab: "active" });
    expect(store.loadError).toContain("Couldn't load your inbox.");
    expect(store.loaded).toBe(false);

    listAnswer = () => page([item("atlas", "a")]);
    await store.reload();
    expect(store.loadError).toBeNull();
    expect(store.items).toHaveLength(1);
  });

  it("names the archive when the archive can't load", async () => {
    const store = new InboxStore();
    listAnswer = () => json({ error: "boom" }, 500);
    await store.show({ agent: null, tab: "archived" });
    expect(store.loadError).toContain("Couldn't load the archive.");
  });

  it("pages older items in, without repeating one already held", async () => {
    const store = new InboxStore();
    listAnswer = (url) =>
      url.searchParams.has("before")
        ? page([item("atlas", "b"), item("atlas", "c")])
        : page([item("atlas", "a"), item("atlas", "b")], "cursor-1");
    await store.show({ agent: null, tab: "active" });

    await store.loadMore();

    expect(requests.at(-1)).toBe("GET /api/hub/inbox?status=active&before=cursor-1&limit=50");
    expect(store.items.map((i) => i.id)).toEqual(["a", "b", "c"]);
    expect(store.nextCursor).toBeNull();
  });

  it("says why older items couldn't load, and keeps the ones held", async () => {
    const store = new InboxStore();
    listAnswer = (url) =>
      url.searchParams.has("before") ? json({}, 503) : page([item("atlas", "a")], "cursor-1");
    await store.show({ agent: null, tab: "active" });

    await store.loadMore();

    expect(store.moreError).toContain("Couldn't load older items.");
    expect(store.items).toHaveLength(1);
    expect(store.nextCursor).toBe("cursor-1");
  });

  it("asks for as many items as it has paged when it fetches again", async () => {
    const store = new InboxStore();
    const many = Array.from({ length: 80 }, (_, n) => item("atlas", `i${String(n)}`));
    listAnswer = () => page(many);
    await store.show({ agent: null, tab: "active" });

    await store.reload();

    expect(requests.at(-1)).toBe("GET /api/hub/inbox?status=active&limit=80");
  });
});

describe("following the counts", () => {
  it("fetches the list again when a count changes while the Inbox is open", async () => {
    const store = new InboxStore();
    await store.show({ agent: null, tab: "active" });
    store.followCounts("atlas=1");
    expect(requests).toHaveLength(1);

    store.followCounts("atlas=2");
    await vi.waitFor(() => {
      expect(requests).toHaveLength(2);
    });

    store.leave();
    store.followCounts("atlas=3");
    expect(requests).toHaveLength(2);
  });
});

describe("opening an item", () => {
  it("marks an unread item read and keeps the hub's copy", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a"), item("scout", "a")]);
    await store.show({ agent: null, tab: "active" });

    expect(await store.open({ agent: "scout", id: "a" })).toBe("shown");

    expect(requests.at(-1)).toBe("PUT /api/hub/inbox/scout/a/read");
    expect(store.items.map((i) => i.read)).toEqual([false, true]);
  });

  it("doesn't ask the hub again for an item already read", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a", { read: true })]);
    await store.show({ agent: null, tab: "active" });

    await store.open({ agent: "atlas", id: "a" });

    expect(requests).toHaveLength(1);
  });

  it("fetches an item the list doesn't hold on its own", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")], "cursor-1");
    await store.show({ agent: null, tab: "active" });

    expect(await store.open({ agent: "scout", id: "old" })).toBe("shown");

    expect(store.openedApart?.id).toBe("old");
    expect(store.find({ agent: "scout", id: "old" })?.read).toBe(true);
  });

  it("reports an item that is gone", async () => {
    const store = new InboxStore();
    await store.show({ agent: null, tab: "active" });
    itemAnswer = () => json({ error: "atlas has no inbox item 'x'" }, 404);

    expect(await store.open({ agent: "atlas", id: "x" })).toBe("missing");
    expect(store.openedApart).toBeNull();
    expect(store.openError).toBeNull();
  });

  it("says why an item couldn't be opened when the hub can't answer", async () => {
    const store = new InboxStore();
    await store.show({ agent: null, tab: "active" });
    itemAnswer = () => json({ error: "boom" }, 500);

    expect(await store.open({ agent: "atlas", id: "x" })).toBe("failed");
    expect(store.openError).toContain("Couldn't open that item.");
  });

  it("keeps the item and says why when marking it read fails", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")]);
    await store.show({ agent: null, tab: "active" });
    itemAnswer = () => json({ error: "boom" }, 500);

    await store.open({ agent: "atlas", id: "a" });

    expect(store.items[0]?.read).toBe(false);
    expect(store.problems["atlas:a"]).toMatchObject({ action: "read" });
    expect(store.problems["atlas:a"]?.message).toContain("Couldn't mark it read.");
  });
});

describe("archiving and restoring", () => {
  it("takes an archived item out of the inbox", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a"), item("atlas", "b")]);
    await store.show({ agent: null, tab: "active" });

    expect(await store.archive({ agent: "atlas", id: "a" })).toBe(true);

    expect(requests.at(-1)).toBe("POST /api/hub/inbox/atlas/a/archive");
    expect(store.items.map((i) => i.id)).toEqual(["b"]);
  });

  it("keeps an item it couldn't archive, says why, and tries again", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")]);
    await store.show({ agent: null, tab: "active" });
    itemAnswer = () => json({ error: "boom" }, 500);

    expect(await store.archive({ agent: "atlas", id: "a" })).toBe(false);
    expect(store.items).toHaveLength(1);
    expect(store.problems["atlas:a"]?.message).toContain("Couldn't archive it.");

    itemAnswer = (_action, agent, id) => json({ item: item(agent, id) });
    expect(await store.retry({ agent: "atlas", id: "a" })).toBe(true);
    expect(requests.at(-1)).toBe("POST /api/hub/inbox/atlas/a/archive");
    expect(store.items).toEqual([]);
    expect(store.problems).toEqual({});
  });

  it("explains an item that was already archived elsewhere", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")]);
    await store.show({ agent: null, tab: "active" });
    itemAnswer = () => json({ error: "gone" }, 404);

    await store.archive({ agent: "atlas", id: "a" });

    expect(store.problems["atlas:a"]?.message).toContain("It may have been archived already.");
  });

  it("takes a restored item out of the archive", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a", { read: true })]);
    await store.show({ agent: null, tab: "archived" });

    await store.restore({ agent: "atlas", id: "a" });

    expect(requests.at(-1)).toBe("POST /api/hub/inbox/atlas/a/restore");
    expect(store.items).toEqual([]);
  });

  it("fetches the inbox again when an Undo brings an item back into it", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "b")]);
    await store.show({ agent: null, tab: "active" });

    listAnswer = () => page([item("atlas", "a"), item("atlas", "b")]);
    await store.restore({ agent: "atlas", id: "a" });

    await vi.waitFor(() => {
      expect(store.items.map((i) => i.id)).toEqual(["a", "b"]);
    });
  });

  it("does one thing at a time to an item", async () => {
    const store = new InboxStore();
    listAnswer = () => page([item("atlas", "a")]);
    await store.show({ agent: null, tab: "active" });

    const first = store.archive({ agent: "atlas", id: "a" });
    expect(store.pending["atlas:a"]).toBe("archive");
    expect(await store.archive({ agent: "atlas", id: "a" })).toBe(false);
    await first;

    expect(requests.filter((r) => r.endsWith("/archive"))).toHaveLength(1);
    expect(store.pending).toEqual({});
  });
});
