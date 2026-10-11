import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { HubInboxItem } from "../src/lib/generated/HubInboxItem";
import type { HubInboxPage } from "../src/lib/generated/HubInboxPage";
import type { HubInboxUnread } from "../src/lib/generated/HubInboxUnread";
import type { UserInboxItem } from "../src/lib/types";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

/** A sample item at a fixed time, `minutes` after 09:00 on 2026-09-30 UTC. */
function item(id: string, minutes: number, read = false): UserInboxItem {
  return {
    id,
    title: `title of ${id}`,
    body: `body of ${id}`,
    source: "agent:test",
    timestamp: new Date(Date.UTC(2026, 8, 30, 9, minutes, 0)).toISOString(),
    read,
    attachments: [],
  };
}

describe("the cross-agent inbox routes", () => {
  let harness: MockServerHarness;

  /** `agent/id` of every item of a page, in order. */
  const listed = (page: HubInboxPage): string[] =>
    page.items.map((entry) => `${entry.agent}/${entry.id}`);

  const get = (path: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}${path}`);

  const send = (method: "PUT" | "POST", path: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}${path}`, { method });

  const page = async (path: string): Promise<HubInboxPage> => {
    const res = await get(path);
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return res.body as HubInboxPage;
  };

  /** Replace an agent's active inbox. */
  const setInbox = (agent: string, items: UserInboxItem[]): void => {
    const found = harness.hub.agents.get(agent);
    if (!found) throw new Error(`no agent ${agent}`);
    found.state.inboxItems = items;
  };

  beforeEach(async () => {
    harness = await startMockServer();
    // scout and atlas are running, drifter stopped and brittle failed.
    setInbox("scout", [item("a", 0), item("same", 2)]);
    setInbox("atlas", [item("same", 2), item("c", 3)]);
    setInbox("drifter", [item("d", 1)]);
    setInbox("brittle", [item("e", 4, true)]);
  });

  afterEach(async () => {
    await harness.close();
  });

  it("merges every agent's items newest first, whatever the agent's state", async () => {
    const result = await page("/api/hub/inbox");
    expect(listed(result)).toEqual([
      "brittle/e",
      "atlas/c",
      "scout/same",
      "atlas/same",
      "drifter/d",
      "scout/a",
    ]);
    expect(result.next_cursor).toBeNull();
  });

  it("describes an item the way the backend does", async () => {
    setInbox("scout", [
      {
        ...item("with-file", 5),
        attachments: [
          {
            filename: "note.txt",
            mime_type: "text/plain",
            size: 5,
            url: "/stale/route",
          },
        ],
      },
    ]);
    const first = (await page("/api/hub/inbox?agent=scout")).items[0];
    expect(first).toEqual({
      agent: "scout",
      id: "with-file",
      title: "title of with-file",
      body: "body of with-file",
      source: "agent:test",
      at: "2026-09-30T09:05:00Z",
      read: false,
      attachments: [
        {
          filename: "note.txt",
          mime_type: "text/plain",
          size: 5,
          url: "/api/agents/scout/inbox/with-file/attachments/0",
        },
      ],
    } satisfies HubInboxItem);
  });

  it("narrows the listing by agent and by status", async () => {
    expect(listed(await page("/api/hub/inbox?agent=drifter"))).toEqual(["drifter/d"]);
    const archived = await page("/api/hub/inbox?status=archived");
    expect(archived.items.length).toBeGreaterThan(0);
    expect(archived.items.every((entry) => entry.read)).toBe(true);
    expect(listed(await page("/api/hub/inbox?status=active&agent=scout"))).toEqual([
      "scout/same",
      "scout/a",
    ]);
  });

  it("answers a listing it can't serve with a JSON error that names the problem", async () => {
    expect(await get("/api/hub/inbox?agent=ghost")).toEqual({
      status: 404,
      body: { error: "no agent named 'ghost'" },
    });
    for (const [query, mentions] of [
      ["status=bogus", "status"],
      ["limit=0", "limit"],
      ["limit=many", "limit"],
      ["limit=-1", "limit"],
      ["before=garbage", "before"],
      ["before=x:scout:id", "before"],
    ] as const) {
      const res = await get(`/api/hub/inbox?${query}`);
      expect(res.status, query).toBe(400);
      expect((res.body as { error: string }).error, query).toContain(mentions);
    }
  });

  it("pages through every item once, in order, even when times and ids tie", async () => {
    const everything = listed(await page("/api/hub/inbox?limit=200"));
    const paged: string[] = [];
    let path = "/api/hub/inbox?limit=2";
    let pages = 0;
    for (;;) {
      const result = await page(path);
      paged.push(...listed(result));
      pages += 1;
      if (result.next_cursor === null) break;
      path = `/api/hub/inbox?limit=2&before=${encodeURIComponent(result.next_cursor)}`;
    }
    expect(pages).toBe(3);
    expect(paged).toEqual(everything);
  });

  it("keeps a cursor working after its item is archived", async () => {
    const first = await page("/api/hub/inbox?limit=1");
    expect(listed(first)).toEqual(["brittle/e"]);
    await send("POST", "/api/hub/inbox/brittle/e/archive");
    const next = await page(
      `/api/hub/inbox?before=${encodeURIComponent(first.next_cursor ?? "")}&limit=1`,
    );
    expect(listed(next)).toEqual(["atlas/c"]);
  });

  it("holds fifty items on a page by default and caps a larger limit at two hundred", async () => {
    setInbox(
      "scout",
      Array.from({ length: 201 }, (_, i) => item(`item_${String(i).padStart(3, "0")}`, i)),
    );
    const defaultPage = await page("/api/hub/inbox?agent=scout");
    expect(defaultPage.items).toHaveLength(50);
    expect(defaultPage.next_cursor).not.toBeNull();

    const capped = await page("/api/hub/inbox?agent=scout&limit=1000");
    expect(capped.items).toHaveLength(200);
    const rest = await page(
      `/api/hub/inbox?agent=scout&limit=1000&before=${encodeURIComponent(capped.next_cursor ?? "")}`,
    );
    expect(rest.items).toHaveLength(1);
    expect(rest.next_cursor).toBeNull();
  });

  it("counts unread items per agent, agents with none included", async () => {
    const res = await get("/api/hub/inbox/unread");
    expect(res).toEqual({
      status: 200,
      body: {
        total: 5,
        by_agent: { scout: 2, atlas: 2, drifter: 1, brittle: 0 },
      } satisfies HubInboxUnread,
    });
    await send("PUT", "/api/hub/inbox/atlas/c/read");
    const after = (await get("/api/hub/inbox/unread")).body as HubInboxUnread;
    expect(after.total).toBe(4);
    expect(after.by_agent.atlas).toBe(1);
  });

  it("marks an item read and answers with it", async () => {
    const res = await send("PUT", "/api/hub/inbox/drifter/d/read");
    expect(res.status).toBe(200);
    expect((res.body as { item: HubInboxItem }).item).toMatchObject({
      agent: "drifter",
      id: "d",
      read: true,
    });
    expect(harness.hub.agents.get("drifter")?.state.inboxItems[0]?.read).toBe(true);
  });

  it("archives and restores an item, moving it between the two lists", async () => {
    const archived = await send("POST", "/api/hub/inbox/scout/a/archive");
    expect(archived.status).toBe(200);
    expect((archived.body as { item: HubInboxItem }).item.id).toBe("a");
    expect(listed(await page("/api/hub/inbox?agent=scout"))).toEqual(["scout/same"]);
    expect(listed(await page("/api/hub/inbox?agent=scout&status=archived"))).toContain("scout/a");

    const restored = await send("POST", "/api/hub/inbox/scout/a/restore");
    expect(restored.status).toBe(200);
    expect(listed(await page("/api/hub/inbox?agent=scout"))).toEqual(["scout/same", "scout/a"]);
    expect(listed(await page("/api/hub/inbox?agent=scout&status=archived"))).not.toContain(
      "scout/a",
    );
  });

  it("shares the archive with the per-agent inbox routes", async () => {
    await send("POST", "/api/hub/inbox/scout/a/archive");
    const archive = await get("/api/agents/scout/inbox/archive");
    expect((archive.body as UserInboxItem[]).map((entry) => entry.id)).toContain("a");

    await send("POST", "/api/agents/scout/inbox/mock_archived_1/restore");
    expect(listed(await page("/api/hub/inbox?agent=scout"))).toContain("scout/mock_archived_1");
  });

  it("marks an archived item read where it is", async () => {
    const archivedId = (await page("/api/hub/inbox?status=archived&agent=scout")).items[0]?.id;
    expect(archivedId).toBeDefined();
    const res = await send("PUT", `/api/hub/inbox/scout/${archivedId ?? ""}/read`);
    expect(res.status).toBe(200);
  });

  it("answers an unknown agent or item with a JSON 404 naming it", async () => {
    const cases = [
      ["PUT", "/api/hub/inbox/ghost/a/read", "no agent named 'ghost'"],
      ["POST", "/api/hub/inbox/ghost/a/archive", "no agent named 'ghost'"],
      ["POST", "/api/hub/inbox/ghost/a/restore", "no agent named 'ghost'"],
      ["PUT", "/api/hub/inbox/scout/nothing/read", "scout has no inbox item 'nothing'"],
      ["POST", "/api/hub/inbox/scout/nothing/archive", "scout has no active inbox item 'nothing'"],
      [
        "POST",
        "/api/hub/inbox/scout/nothing/restore",
        "scout has no archived inbox item 'nothing'",
      ],
      ["POST", "/api/hub/inbox/scout/a/restore", "scout has no archived inbox item 'a'"],
    ] as const;
    for (const [method, path, error] of cases) {
      expect(await send(method, path), `${method} ${path}`).toEqual({
        status: 404,
        body: { error },
      });
    }
  });

  it("refuses an id that isn't a bare item id", async () => {
    for (const id of ["..%2Fsecret", "a%2Fb", "a%5Cb"]) {
      const res = await send("PUT", `/api/hub/inbox/scout/${id}/read`);
      expect(res.status, id).toBe(400);
      expect((res.body as { error: string }).error, id).toContain("isn't an inbox item id");
    }
  });

  it("won't move an item onto a different one with the same id", async () => {
    await send("POST", "/api/hub/inbox/scout/a/archive");
    harness.hub.agents.get("scout")?.state.inboxItems.push(item("a", 30));

    const archive = await send("POST", "/api/hub/inbox/scout/a/archive");
    const restore = await send("POST", "/api/hub/inbox/scout/a/restore");

    expect(archive.status).toBe(409);
    expect(restore.status).toBe(409);
    expect((archive.body as { error: string }).error).toContain("already holds a different item");
    expect(harness.hub.agents.get("scout")?.state.inboxItems.map((i) => i.id)).toEqual([
      "same",
      "a",
    ]);
  });
});
