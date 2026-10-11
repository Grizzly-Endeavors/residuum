import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { UserInboxAttachment, UserInboxItem } from "../src/lib/types";
import { attachmentUrl, inboxRoutes } from "./inbox";
import {
  fetchJson,
  fetchText,
  startMockServer,
  startRouteHarness,
  type MockServerHarness,
  type RouteHarness,
} from "./test-support";

const REPORT: UserInboxAttachment = {
  filename: 'q3 "final".txt',
  mime_type: "text/plain",
  size: 12,
  url: "",
};
const CHART: UserInboxAttachment = {
  filename: "chart.png",
  mime_type: "image/png",
  size: 70,
  url: "",
};

describe("inbox routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(inboxRoutes);
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/inbox${path}`;
  const post = (path: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(url(path), { method: "POST" });
  const ids = (body: unknown): string[] => (body as UserInboxItem[]).map((item) => item.id);

  describe("listing", () => {
    it("lists the inbox newest first, with the minute-precision timestamp the backend serializes", async () => {
      const { status, body } = await fetchJson(url(""));
      expect(status).toBe(200);
      expect(ids(body)).toEqual(["mock_1", "mock_2"]);
      for (const item of body as UserInboxItem[]) {
        expect(item.timestamp).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/);
        expect(item.attachments).toEqual([]);
      }
    });

    it("orders by timestamp, not by where an item sits in the state", async () => {
      const [first, second] = harness.state.inboxItems;
      if (first === undefined || second === undefined) throw new Error("the sample inbox is empty");
      harness.state.inboxItems = [second, first];
      expect(ids((await fetchJson(url(""))).body)).toEqual(["mock_1", "mock_2"]);
    });

    it("lists the sample archive, with the same shape as the inbox", async () => {
      const { status, body } = await fetchJson(url("/archive"));
      expect(status).toBe(200);
      expect(body).toEqual([
        {
          id: "mock_archived_1",
          title: "Last week's digest",
          body: "Here was last week's summary.",
          source: "agent:digest",
          timestamp: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/) as unknown,
          read: true,
          attachments: [],
        },
      ]);
    });

    it("lists an agent that has never run as empty, inbox and archive alike", async () => {
      harness.state.inboxItems = [];
      harness.state.inboxArchive = [];
      expect(await fetchJson(url(""))).toEqual({ status: 200, body: [] });
      expect(await fetchJson(url("/archive"))).toEqual({ status: 200, body: [] });
    });
  });

  describe("reading", () => {
    it("marks an item read and answers with it", async () => {
      const res = await fetchJson(url("/mock_1/read"), { method: "PUT" });
      expect(res.status).toBe(200);
      expect(res.body).toMatchObject({ id: "mock_1", read: true, title: "Deploy tomorrow" });
      const listed = (await fetchJson(url(""))).body as UserInboxItem[];
      expect(listed.find((item) => item.id === "mock_1")?.read).toBe(true);
    });

    it("takes the id with or without .json", async () => {
      expect((await fetchJson(url("/mock_1.json/read"), { method: "PUT" })).status).toBe(200);
    });

    it("answers 500 for an item that isn't in the inbox, as the backend does", async () => {
      expect(await fetchText(url("/ghost/read"), { method: "PUT" })).toEqual({
        status: 500,
        body: "failed to mark inbox item as read: failed to load inbox item ghost.json for mark_read",
      });
    });
  });

  describe("archiving and restoring", () => {
    it("moves an item to the archive and answers null", async () => {
      expect(await post("/mock_1/archive")).toEqual({ status: 200, body: null });
      expect(ids((await fetchJson(url(""))).body)).toEqual(["mock_2"]);
      expect(ids((await fetchJson(url("/archive"))).body)).toEqual(["mock_1", "mock_archived_1"]);
    });

    it("brings an archived item back as it was", async () => {
      await post("/mock_1/archive");
      expect(await post("/mock_1/restore")).toEqual({ status: 200, body: null });
      const restored = (await fetchJson(url(""))).body as UserInboxItem[];
      expect(restored.map((item) => item.id)).toEqual(["mock_1", "mock_2"]);
      expect(restored[0]).toMatchObject({ read: false, title: "Deploy tomorrow" });
      expect(ids((await fetchJson(url("/archive"))).body)).toEqual(["mock_archived_1"]);
    });

    it("lists the archive newest first", async () => {
      await post("/mock_2/archive");
      await post("/mock_1/archive");
      expect(ids((await fetchJson(url("/archive"))).body)).toEqual([
        "mock_1",
        "mock_2",
        "mock_archived_1",
      ]);
    });

    it("answers 500 for an item that isn't there, as the backend does", async () => {
      expect(await fetchText(url("/ghost/archive"), { method: "POST" })).toEqual({
        status: 500,
        body: "failed to archive inbox item: inbox item 'ghost.json' not found or could not be archived",
      });
      expect(await fetchText(url("/mock_1/restore"), { method: "POST" })).toEqual({
        status: 500,
        body: "failed to restore inbox item: inbox item 'mock_1.json' not found in archive or could not be restored",
      });
    });

    it("can't mark an archived item read, or archive it twice", async () => {
      await post("/mock_1/archive");
      expect((await fetchText(url("/mock_1/read"), { method: "PUT" })).status).toBe(500);
      expect((await fetchText(url("/mock_1/archive"), { method: "POST" })).status).toBe(500);
    });
  });

  describe("attachments", () => {
    beforeEach(() => {
      const item = harness.state.inboxItems.find((candidate) => candidate.id === "mock_1");
      if (item === undefined) throw new Error("the sample inbox has no mock_1");
      item.attachments = [REPORT, CHART];
    });

    it("lists each attachment with the route that serves it under the agent", async () => {
      const item = ((await fetchJson(url(""))).body as UserInboxItem[])[0];
      expect(item?.attachments).toEqual([
        { ...REPORT, url: "/api/agents/atlas/inbox/mock_1/attachments/0" },
        { ...CHART, url: "/api/agents/atlas/inbox/mock_1/attachments/1" },
      ]);
      expect(item?.attachments[1]?.url).toBe(attachmentUrl("atlas", "mock_1", 1));
    });

    it("serves an attachment by its position, inline, under its type", async () => {
      const res = await fetch(url("/mock_1/attachments/0"));
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toBe("text/plain");
      expect(res.headers.get("content-disposition")).toBe('inline; filename="q3 final.txt"');
      expect(await res.text()).toContain('q3 "final".txt');
    });

    it("serves an image attachment as a PNG", async () => {
      const res = await fetch(url("/mock_1/attachments/1"));
      expect(res.headers.get("content-type")).toBe("image/png");
      const bytes = new Uint8Array(await res.arrayBuffer());
      expect(Array.from(bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47]);
    });

    it("keeps serving an item's attachments after it is archived", async () => {
      await post("/mock_1/archive");
      const res = await fetch(url("/mock_1/attachments/0"));
      expect(res.status).toBe(200);
      const archived = ((await fetchJson(url("/archive"))).body as UserInboxItem[])[0];
      expect(archived?.attachments[0]?.url).toBe("/api/agents/atlas/inbox/mock_1/attachments/0");
    });

    it("answers 404 for an item or position that isn't there, and 400 for a position that isn't a number", async () => {
      expect((await fetch(url("/ghost/attachments/0"))).status).toBe(404);
      expect((await fetch(url("/mock_1/attachments/2"))).status).toBe(404);
      expect((await fetch(url("/mock_2/attachments/0"))).status).toBe(404);
      expect((await fetch(url("/mock_1/attachments/first"))).status).toBe(400);
    });
  });
});

describe("inbox routes over the whole mock", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer();
  });

  afterEach(async () => {
    await mock.close();
  });

  const at = (agent: string, path: string): string =>
    `${mock.baseUrl}/api/agents/${agent}/inbox${path}`;

  it("keeps every agent's inbox its own", async () => {
    await fetchJson(at("atlas", "/mock_1/archive"), { method: "POST" });
    const atlas = (await fetchJson(at("atlas", ""))).body as UserInboxItem[];
    const scout = (await fetchJson(at("scout", ""))).body as UserInboxItem[];
    expect(atlas.map((item) => item.id)).toEqual(["mock_2"]);
    expect(scout.map((item) => item.id)).toEqual(["mock_1", "mock_2"]);
    const archive = (await fetchJson(at("atlas", "/archive"))).body as UserInboxItem[];
    expect(archive.map((item) => item.id)).toEqual(["mock_1", "mock_archived_1"]);
    const scoutArchive = (await fetchJson(at("scout", "/archive"))).body as UserInboxItem[];
    expect(scoutArchive.map((item) => item.id)).toEqual(["mock_archived_1"]);
  });

  it.each(["drifter", "brittle"])(
    "answers for %s, which has never run, with empty listings and the backend's errors",
    async (agent) => {
      expect(await fetchJson(at(agent, ""))).toEqual({ status: 200, body: [] });
      expect(await fetchJson(at(agent, "/archive"))).toEqual({ status: 200, body: [] });
      expect((await fetchText(at(agent, "/mock_1/restore"), { method: "POST" })).status).toBe(500);
      expect((await fetchText(at(agent, "/mock_1/archive"), { method: "POST" })).status).toBe(500);
      expect((await fetch(at(agent, "/mock_1/attachments/0"))).status).toBe(404);
    },
  );

  it("serves an agent's inbox routes while it is stopped", async () => {
    await fetchJson(`${mock.baseUrl}/api/hub/agents/atlas/stop`, { method: "POST" });
    expect(await fetchJson(at("atlas", "/mock_1/archive"), { method: "POST" })).toEqual({
      status: 200,
      body: null,
    });
    expect(((await fetchJson(at("atlas", "/archive"))).body as unknown[]).length).toBe(2);
    expect(await fetchJson(at("atlas", "/mock_1/restore"), { method: "POST" })).toEqual({
      status: 200,
      body: null,
    });
    expect((await fetch(at("atlas", "/mock_1/attachments/0"))).status).toBe(404);
  });
});
