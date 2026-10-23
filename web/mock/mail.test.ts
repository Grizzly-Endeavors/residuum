import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  MailMessage,
  MailThread,
  MailThreadPage,
  MailThreadSummary,
  OverviewResponse,
} from "../src/lib/hub-types";
import { MAIL_SEED } from "./data/mail";
import { MAX_UPLOAD_BYTES } from "./mail";
import { replyTitle } from "./mail-threads";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";
import type { MockAgent } from "./state";

/** The threads of the scenario by subject, for a test to name them without their ids. */
const WIKI = MAIL_SEED.wikiAudit;

describe("the mail routes", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  const url = (path: string): string => `${mock.baseUrl}${path}`;
  const get = (path: string): Promise<{ status: number; body: unknown }> => fetchJson(url(path));
  const post = (path: string, body?: unknown): Promise<{ status: number; body: unknown }> =>
    fetchJson(url(path), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  /** The status of a thread action, which answers with no body. */
  const act = async (id: string, action: "read" | "archive" | "restore"): Promise<number> =>
    (await fetch(url(threadPath(id, action)), { method: "POST" })).status;
  const threadPath = (id: string, action?: string): string =>
    `/api/hub/mail/threads/${encodeURIComponent(id)}${action === undefined ? "" : `/${action}`}`;

  const agent = (name: string): MockAgent => {
    const found = mock.hub.agents.get(name);
    if (!found) throw new Error(`no agent ${name}`);
    return found;
  };

  const page = async (query: string): Promise<MailThreadPage> => {
    const res = await get(`/api/hub/mail/threads?${query}`);
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return res.body as MailThreadPage;
  };

  /** The id of every thread of a page, in order. */
  const listed = async (query: string): Promise<string[]> =>
    (await page(query)).threads.map((thread) => thread.thread_id);

  const thread = async (id: string): Promise<MailThread> => {
    const res = await get(threadPath(id));
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return res.body as MailThread;
  };

  const summary = async (folder: string, id: string): Promise<MailThreadSummary | undefined> =>
    (await page(`folder=${folder}`)).threads.find((entry) => entry.thread_id === id);

  /** Stage a file the way the client does and answer its id. */
  const upload = async (
    filename: string,
    bytes: Uint8Array<ArrayBuffer> | string,
  ): Promise<string> => {
    const res = await fetchJson(url(`/api/hub/uploads?filename=${encodeURIComponent(filename)}`), {
      method: "POST",
      headers: { "Content-Type": "application/octet-stream" },
      body: bytes,
    });
    expect(res.status, JSON.stringify(res.body)).toBe(200);
    return (res.body as { id: string }).id;
  };

  const unreadOf = async (name: string): Promise<number> => {
    const overview = (await get("/api/hub/overview")).body as OverviewResponse;
    return overview.agents.find((entry) => entry.name === name)?.inbox_unread ?? -1;
  };

  describe("listing", () => {
    it("lists the Inbox newest first: the agents' own messages, then the scenario's threads", async () => {
      expect(await listed("folder=inbox")).toEqual([
        MAIL_SEED.deployTomorrow.threadIds.scout,
        MAIL_SEED.deployTomorrow.threadIds.atlas,
        MAIL_SEED.dailyDigest.threadIds.scout,
        MAIL_SEED.dailyDigest.threadIds.atlas,
        MAIL_SEED.syncResearch.threadId,
        WIKI.threadId,
      ]);
    });

    it("lists Sent as the threads with a message you wrote, and Archived as those with only archived messages from agents", async () => {
      expect(await listed("folder=sent")).toEqual([
        MAIL_SEED.changelog.threadId,
        WIKI.threadId,
        MAIL_SEED.backupRotation.threadId,
      ]);
      expect(await listed("folder=archived")).toEqual([
        MAIL_SEED.backupRotation.threadId,
        MAIL_SEED.lastWeeksDigest.threadIds.scout,
        MAIL_SEED.lastWeeksDigest.threadIds.atlas,
      ]);
    });

    it("defaults to the Inbox", async () => {
      expect(await listed("")).toEqual(await listed("folder=inbox"));
    });

    it("keeps a thread in the Inbox and in Sent at once", async () => {
      expect(await summary("inbox", WIKI.threadId)).toMatchObject({
        in_inbox: true,
        in_sent: true,
      });
      expect(await summary("sent", WIKI.threadId)).toMatchObject({ in_inbox: true, in_sent: true });
    });

    it("narrows the listing to the threads an agent took part in", async () => {
      expect(await listed("folder=sent&agent=drifter")).toEqual([MAIL_SEED.changelog.threadId]);
      expect(await listed("folder=inbox&agent=scout")).toEqual([
        MAIL_SEED.deployTomorrow.threadIds.scout,
        MAIL_SEED.dailyDigest.threadIds.scout,
        MAIL_SEED.syncResearch.threadId,
        WIKI.threadId,
      ]);
      expect(await listed("folder=inbox&agent=drifter")).toEqual([]);
    });

    it("describes a thread by its first message and its latest", async () => {
      expect(await summary("inbox", WIKI.threadId)).toEqual({
        thread_id: WIKI.threadId,
        subject: WIKI.subject,
        participants: ["atlas", "scout"],
        message_count: 4,
        latest_at: "2026-03-12T09:20:00Z",
        snippet:
          "Thanks both. Atlas, please archive the two oldest pages. Scout, hold off on the project pages until atlas is done.",
        unread: false,
        has_attachments: true,
        in_inbox: true,
        in_sent: true,
        archived: false,
      } satisfies MailThreadSummary);
    });

    it("flags a thread unread while an active message from an agent is", async () => {
      const unread = (await page("folder=inbox")).threads.filter((entry) => entry.unread);
      expect(unread.map((entry) => entry.thread_id)).toEqual([
        MAIL_SEED.deployTomorrow.threadIds.scout,
        MAIL_SEED.deployTomorrow.threadIds.atlas,
      ]);
    });

    it("makes the snippet one plain line of at most 140 characters, without Markdown markers", async () => {
      const research = await summary("inbox", MAIL_SEED.syncResearch.threadId);
      expect(research?.snippet.startsWith("Local-first sync: what I found I compared four")).toBe(
        true,
      );
      expect(research?.snippet).not.toMatch(/[#*`\n]/);
      expect(Array.from(research?.snippet ?? "")).toHaveLength(140);
    });

    it("works while every agent is stopped", async () => {
      for (const name of ["scout", "atlas"]) {
        await post(`/api/hub/agents/${name}/stop`);
      }
      expect((await page("folder=inbox")).threads).toHaveLength(6);
      expect((await get(threadPath(WIKI.threadId))).status).toBe(200);
      expect(await act(WIKI.threadId, "read")).toBe(204);
    });

    it("pages through every thread once, newest first, with the cursor of the page before", async () => {
      const everything = await listed("folder=inbox&limit=200");
      const paged: string[] = [];
      let query = "folder=inbox&limit=2";
      let pages = 0;
      for (;;) {
        const result = await page(query);
        paged.push(...result.threads.map((entry) => entry.thread_id));
        pages += 1;
        if (result.next === null) break;
        query = `folder=inbox&limit=2&before=${encodeURIComponent(result.next)}`;
      }
      expect(pages).toBe(3);
      expect(paged).toEqual(everything);
    });

    it("keeps a cursor working after its thread leaves the folder", async () => {
      const first = await page("folder=inbox&limit=1");
      expect(await act(first.threads[0]?.thread_id ?? "", "archive")).toBe(204);
      const next = await page(
        `folder=inbox&limit=1&before=${encodeURIComponent(first.next ?? "")}`,
      );
      expect(next.threads.map((entry) => entry.thread_id)).toEqual([
        MAIL_SEED.deployTomorrow.threadIds.atlas,
      ]);
    });

    it("caps a larger limit at two hundred and answers 400 for parameters it can't use", async () => {
      expect((await page("folder=inbox&limit=1000")).threads).toHaveLength(6);
      for (const [query, mentions] of [
        ["folder=trash", "folder"],
        ["limit=0", "limit"],
        ["limit=many", "limit"],
        ["limit=-1", "limit"],
        ["before=garbage", "before"],
        ["agent=ghost", "ghost"],
      ] as const) {
        const res = await get(`/api/hub/mail/threads?${query}`);
        expect(res.status, query).toBe(400);
        expect((res.body as { error: string }).error, query).toContain(mentions);
      }
    });
  });

  describe("one thread", () => {
    it("lists a thread's messages oldest first, with who they came from, who they went to and who has seen them", async () => {
      const result = await thread(WIKI.threadId);
      expect(result).toMatchObject({
        thread_id: WIKI.threadId,
        subject: WIKI.subject,
        participants: ["atlas", "scout"],
        recipients_for_reply: ["atlas", "scout"],
      });
      expect(result.messages.map((message) => message.ref)).toEqual([
        `atlas/agent/${WIKI.sentId}`,
        `atlas/user/${WIKI.replyId}`,
        `scout/user/${WIKI.replyId}`,
        `atlas/agent/${WIKI.answerId}`,
      ]);
      const [sent, atlasReply, scoutReply, answer] = result.messages;
      expect(sent).toMatchObject({
        from: { kind: "user" },
        to: ["atlas", "scout"],
        title: WIKI.subject,
        in_reply_to: null,
        read: true,
        archived: false,
        seen_by: [
          { agent: "atlas", seen: true },
          { agent: "scout", seen: true },
        ],
      } satisfies Partial<MailMessage>);
      expect(atlasReply).toMatchObject({
        from: { kind: "agent", agent: "atlas" },
        to: [],
        title: `Re: ${WIKI.subject}`,
        in_reply_to: `atlas/agent/${WIKI.sentId}`,
        seen_by: [],
      } satisfies Partial<MailMessage>);
      expect(scoutReply?.in_reply_to).toBe(`scout/agent/${WIKI.sentId}`);
      expect(answer).toMatchObject({
        from: { kind: "user" },
        to: ["atlas", "scout"],
        in_reply_to: `scout/user/${WIKI.replyId}`,
        seen_by: [
          { agent: "atlas", seen: true },
          { agent: "scout", seen: false },
        ],
      } satisfies Partial<MailMessage>);
    });

    it("opens a thread from the ref of any of its messages, and names it by its own id", async () => {
      for (const ref of [
        `atlas/agent/${WIKI.sentId}`,
        `scout/agent/${WIKI.sentId}`,
        `scout/user/${WIKI.replyId}`,
        `scout/agent/${WIKI.answerId}`,
      ]) {
        expect((await thread(ref)).thread_id, ref).toBe(WIKI.threadId);
      }
    });

    it("gives each attachment where the bytes are and where the file panel finds the file", async () => {
      const result = await thread(WIKI.threadId);
      expect(result.messages[0]?.attachments).toEqual([
        {
          filename: "audit-scope.md",
          size: expect.any(Number) as number,
          content_type: "text/markdown",
          url: `/api/agents/atlas/inbox/${WIKI.sentId}/attachments/0`,
          agent: "atlas",
          path: `inbox/agent/attachments/${WIKI.sentId}/audit-scope.md`,
        },
      ]);
      expect(result.messages[2]?.attachments[0]).toMatchObject({
        filename: "source-notes.pdf",
        content_type: "application/pdf",
        agent: "scout",
        path: `inbox/user/attachments/${WIKI.replyId}/source-notes.pdf`,
      });
    });

    it("answers 404 for a thread it doesn't have, and 400 for an id that isn't percent-encoded properly", async () => {
      expect(await get(threadPath("scout/user/nothing"))).toEqual({
        status: 404,
        body: { error: "there is no mail thread 'scout/user/nothing'" },
      });
      for (const action of ["read", "archive", "restore"] as const) {
        expect(await act("scout/user/nothing", action), action).toBe(404);
      }
      expect((await get("/api/hub/mail/threads/%E0%A4%A")).status).toBe(400);
    });

    it("leaves out what isn't mail: items from pulses, actions and artifacts", async () => {
      const before = (await page("folder=sent&limit=200")).threads.length;
      agent("scout").state.agentInbox.push(
        {
          id: "pulse-1",
          title: "Morning pulse",
          body: "Nothing to report.",
          source: "pulse",
          timestamp: mock.hub.env.clock.iso(),
        },
        {
          id: "artifact-1",
          title: "Tip report",
          body: "Split 3 ways.",
          source: "artifact:tip-splitter",
          timestamp: mock.hub.env.clock.iso(),
        },
      );
      expect((await page("folder=sent&limit=200")).threads).toHaveLength(before);
    });

    it("starts a thread of its own from a reply to something that isn't mail", async () => {
      agent("scout").state.inboxItems.push({
        id: "orphan",
        title: "Re: a pulse",
        body: "Following up.",
        source: "agent",
        timestamp: mock.hub.env.clock.iso(),
        read: false,
        attachments: [],
        in_reply_to: "scout/agent/a-pulse-nobody-wrote",
      });
      const result = await thread("scout/user/orphan");
      expect(result.messages).toHaveLength(1);
    });
  });

  describe("opening, archiving and restoring", () => {
    it("reads every message an agent sent in the thread, archived ones too, and tells the overview", async () => {
      expect(await unreadOf("scout")).toBe(1);
      const scoutArchive = agent("scout").state.inboxArchive;
      const archived = scoutArchive[0];
      if (archived === undefined) throw new Error("scout has no archived item");
      archived.read = false;

      expect(await act(MAIL_SEED.deployTomorrow.threadIds.scout, "read")).toBe(204);
      expect(agent("scout").state.inboxItems.find((item) => item.id === "mock_1")?.read).toBe(true);
      expect(await summary("inbox", MAIL_SEED.deployTomorrow.threadIds.scout)).toMatchObject({
        unread: false,
      });
      await act(MAIL_SEED.lastWeeksDigest.threadIds.scout, "read");
      expect(archived.read).toBe(true);
      // atlas's own copy is a different message.
      expect(agent("atlas").state.inboxItems.find((item) => item.id === "mock_1")?.read).toBe(
        false,
      );
    });

    it("archives the thread's active messages from agents and leaves the agents' copies of yours", async () => {
      const atlas = agent("atlas").state;
      const copiesBefore = JSON.stringify(atlas.agentInbox);

      expect(await act(WIKI.threadId, "archive")).toBe(204);

      expect(atlas.inboxArchive.map((item) => item.id)).toContain(WIKI.replyId);
      expect(atlas.inboxItems.map((item) => item.id)).not.toContain(WIKI.replyId);
      expect(agent("scout").state.inboxArchive.map((item) => item.id)).toContain(WIKI.replyId);
      expect(JSON.stringify(atlas.agentInbox)).toBe(copiesBefore);

      expect(await summary("archived", WIKI.threadId)).toMatchObject({
        archived: true,
        in_inbox: false,
        in_sent: true,
      });
      expect(await summary("inbox", WIKI.threadId)).toBeUndefined();
      expect(await summary("sent", WIKI.threadId)).toMatchObject({ archived: true });
      const archivedThread = await thread(WIKI.threadId);
      expect(archivedThread.messages.map((message) => message.archived)).toEqual([
        false,
        true,
        true,
        false,
      ]);
    });

    it("restores the thread to the Inbox, and an unread message counts again", async () => {
      await act(MAIL_SEED.deployTomorrow.threadIds.atlas, "archive");
      expect(await unreadOf("atlas")).toBe(0);
      expect(await summary("archived", MAIL_SEED.deployTomorrow.threadIds.atlas)).toMatchObject({
        unread: false,
      });

      expect(await act(MAIL_SEED.deployTomorrow.threadIds.atlas, "restore")).toBe(204);

      expect(await unreadOf("atlas")).toBe(1);
      expect(await summary("inbox", MAIL_SEED.deployTomorrow.threadIds.atlas)).toMatchObject({
        unread: true,
        in_inbox: true,
        archived: false,
      });
      expect(await summary("archived", MAIL_SEED.deployTomorrow.threadIds.atlas)).toBeUndefined();
    });

    it("does nothing, and says so with 204, for a thread with no message from an agent or nothing to move", async () => {
      expect(await act(MAIL_SEED.changelog.threadId, "archive")).toBe(204);
      expect(await summary("sent", MAIL_SEED.changelog.threadId)).toMatchObject({
        in_inbox: false,
        archived: false,
        in_sent: true,
      });
      expect(await act(WIKI.threadId, "restore")).toBe(204);
      expect(await act(WIKI.threadId, "archive")).toBe(204);
      expect(await act(WIKI.threadId, "archive")).toBe(204);
      expect(
        agent("atlas").state.inboxArchive.filter((item) => item.id === WIKI.replyId),
      ).toHaveLength(1);
    });

    it("brings an archived thread back to the Inbox when an agent answers it", async () => {
      expect(await summary("archived", MAIL_SEED.backupRotation.threadId)).toMatchObject({
        archived: true,
      });

      const added = await post("/api/mock/user-inbox-add?agent=atlas", {
        body: "Done: backups now keep 14 nights.",
        in_reply_to: `atlas/user/${MAIL_SEED.backupRotation.replyId}`,
      });
      expect(added.status).toBe(200);

      expect(await summary("archived", MAIL_SEED.backupRotation.threadId)).toBeUndefined();
      expect(await summary("inbox", MAIL_SEED.backupRotation.threadId)).toMatchObject({
        message_count: 3,
        unread: true,
        in_inbox: true,
        archived: false,
        in_sent: true,
      });
      const result = await thread(MAIL_SEED.backupRotation.threadId);
      expect(result.messages.map((message) => [message.title, message.archived])).toEqual([
        ["Backup rotation", false],
        ["Re: Backup rotation", true],
        ["Re: Backup rotation", false],
      ]);
    });
  });

  describe("sending", () => {
    it("writes one copy per recipient with one id, and presents them as one message to the agents it went to", async () => {
      const staged = await upload("notes.txt", "remember the milk\n");

      const sent = await post("/api/hub/mail/send", {
        to: ["scout", "atlas", "scout"],
        subject: "Groceries",
        body: "Please **add** this to the wiki.",
        attachments: [staged],
      });

      expect(sent.status, JSON.stringify(sent.body)).toBe(200);
      const { thread_id } = sent.body as { thread_id: string };
      expect(thread_id).toBe("atlas/agent/20260314_groceries");
      for (const name of ["atlas", "scout"]) {
        const copies = agent(name).state.agentInbox.filter(
          (item) => item.id === "20260314_groceries",
        );
        expect(copies, name).toEqual([
          {
            id: "20260314_groceries",
            title: "Groceries",
            body: "Please **add** this to the wiki.",
            source: "web",
            timestamp: "2026-03-14T12:00:00.000Z",
            attachments: [{ filename: "notes.txt", mime_type: "text/plain", size: 18, url: "" }],
          },
        ]);
      }
      expect(agent("drifter").state.agentInbox.map((item) => item.id)).not.toContain(
        "20260314_groceries",
      );
      const result = await thread(thread_id);
      expect(result.messages).toHaveLength(1);
      expect(result.messages[0]).toMatchObject({
        ref: thread_id,
        from: { kind: "user" },
        to: ["atlas", "scout"],
        title: "Groceries",
        in_reply_to: null,
        seen_by: [
          { agent: "atlas", seen: false },
          { agent: "scout", seen: false },
        ],
      });
      expect(result.recipients_for_reply).toEqual(["atlas", "scout"]);
      expect(await summary("sent", thread_id)).toMatchObject({
        in_sent: true,
        in_inbox: false,
        archived: false,
        unread: false,
        has_attachments: true,
        message_count: 1,
      });
    });

    it("shows a copy an agent has read or archived as seen", async () => {
      const sent = await post("/api/hub/mail/send", {
        to: ["atlas", "scout"],
        subject: "Hi",
        body: "Hi",
        attachments: [],
      });
      const { thread_id } = sent.body as { thread_id: string };
      const [atlasCopy] = agent("atlas").state.agentInbox.filter((item) => item.title === "Hi");
      const [scoutCopy] = agent("scout").state.agentInbox.filter((item) => item.title === "Hi");
      if (atlasCopy === undefined || scoutCopy === undefined) throw new Error("no copies");
      atlasCopy.read = true;
      scoutCopy.archived = true;

      expect((await thread(thread_id)).messages[0]?.seen_by).toEqual([
        { agent: "atlas", seen: true },
        { agent: "scout", seen: true },
      ]);
      atlasCopy.read = false;
      scoutCopy.archived = false;
      expect((await thread(thread_id)).messages[0]?.seen_by).toEqual([
        { agent: "atlas", seen: false },
        { agent: "scout", seen: false },
      ]);
    });

    it("moves the staged file into the attachments: served by the attachment route, open by path, and no longer staged", async () => {
      const text = await upload("notes.md", "# Notes\n\nA list.\n");
      const binary = Uint8Array.from([0, 255, 1, 254, 2, 253]);
      const staged = await upload("blob.bin", binary);
      const { thread_id } = (
        await post("/api/hub/mail/send", {
          to: ["scout"],
          subject: "Two files",
          body: "Here.",
          attachments: [text, staged],
        })
      ).body as { thread_id: string };
      const id = "20260314_two_files";

      const asText = await fetch(url(`/api/agents/scout/inbox/${id}/attachments/0`));
      expect(asText.status).toBe(200);
      expect(asText.headers.get("content-type")).toBe("text/markdown");
      expect(await asText.text()).toBe("# Notes\n\nA list.\n");
      const asBinary = await fetch(url(`/api/agents/scout/inbox/${id}/attachments/1`));
      expect(asBinary.headers.get("content-type")).toBe("application/octet-stream");
      expect(Buffer.from(await asBinary.arrayBuffer())).toEqual(Buffer.from(binary));

      const file = await fetch(
        url(
          `/api/agents/scout/workspace/file?path=${encodeURIComponent(`inbox/agent/attachments/${id}/notes.md`)}`,
        ),
      );
      expect(await file.text()).toBe("# Notes\n\nA list.\n");
      const attachments = (await thread(thread_id)).messages[0]?.attachments ?? [];
      expect(attachments.map((attachment) => attachment.path)).toEqual([
        `inbox/agent/attachments/${id}/notes.md`,
        `inbox/agent/attachments/${id}/blob.bin`,
      ]);

      const again = await post("/api/hub/mail/send", {
        to: ["scout"],
        subject: "Again",
        body: "Again.",
        attachments: [text],
      });
      expect(again.status).toBe(400);
    });

    it("serves the attachments of every recipient's copy", async () => {
      const staged = await upload("notes.txt", "shared\n");
      await post("/api/hub/mail/send", {
        to: ["atlas", "scout"],
        subject: "Shared",
        body: "Look.",
        attachments: [staged],
      });
      for (const name of ["atlas", "scout"]) {
        const res = await fetch(url(`/api/agents/${name}/inbox/20260314_shared/attachments/0`));
        expect(await res.text(), name).toBe("shared\n");
      }
    });

    it("gives two files of one name distinct names", async () => {
      const first = await upload("report.pdf", "one");
      const second = await upload("report.pdf", "two");
      const { thread_id } = (
        await post("/api/hub/mail/send", {
          to: ["scout"],
          subject: "Reports",
          body: "Both.",
          attachments: [first, second],
        })
      ).body as { thread_id: string };
      const names = (await thread(thread_id)).messages[0]?.attachments.map((a) => a.filename);
      expect(names).toEqual(["report.pdf", "report (2).pdf"]);
    });

    it("numbers the id of a subject the day already has, across every recipient's inbox", async () => {
      const ids: string[] = [];
      for (const to of [["scout"], ["atlas"], ["atlas", "scout"]]) {
        const sent = await post("/api/hub/mail/send", {
          to,
          subject: "Same",
          body: "Hi",
          attachments: [],
        });
        ids.push((sent.body as { thread_id: string }).thread_id);
      }
      expect(ids).toEqual([
        "scout/agent/20260314_same",
        "atlas/agent/20260314_same_2",
        "atlas/agent/20260314_same_3",
      ]);
    });

    it("sends to a stopped agent, and lets the subject be empty", async () => {
      const sent = await post("/api/hub/mail/send", {
        to: ["drifter"],
        subject: "",
        body: "Wake up.",
        attachments: [],
      });
      const { thread_id } = sent.body as { thread_id: string };
      expect(thread_id).toBe("drifter/agent/20260314");
      expect((await thread(thread_id)).subject).toBe("");
      expect(await summary("sent", thread_id)).toMatchObject({ subject: "", snippet: "Wake up." });
    });

    it("answers 400 for a message it can't send, naming why", async () => {
      const staged = await upload("a.txt", "a");
      const cases: [unknown, string][] = [
        [{ to: ["scout"], subject: "Hi", body: "   \n", attachments: [] }, "blank"],
        [{ to: [], subject: "Hi", body: "Hi", attachments: [] }, "at least one agent"],
        [{ to: ["scout", "ghost"], subject: "Hi", body: "Hi", attachments: [] }, "ghost"],
        [{ to: ["scout"], subject: "Hi", body: "Hi", attachments: ["upload_9999"] }, "not staged"],
      ];
      for (const [body, mentions] of cases) {
        const res = await post("/api/hub/mail/send", body);
        expect(res.status, mentions).toBe(400);
        expect((res.body as { error: string }).error, mentions).toContain(mentions);
      }
      // Nothing was written, and the file a refused send named is still staged.
      expect(agent("scout").state.agentInbox.filter((item) => item.title === "Hi")).toEqual([]);
      const ok = await post("/api/hub/mail/send", {
        to: ["scout"],
        subject: "Hi",
        body: "Hi",
        attachments: [staged],
      });
      expect(ok.status).toBe(200);
    });

    it("answers 422 for a body without the fields it needs and 400 for one that isn't JSON", async () => {
      for (const body of [
        {},
        { to: "scout", subject: "", body: "x" },
        { to: [], subject: 1, body: "x" },
        { to: [], subject: "", body: "x", attachments: [1] },
      ]) {
        expect((await post("/api/hub/mail/send", body)).status, JSON.stringify(body)).toBe(422);
      }
      const res = await fetchJson(url("/api/hub/mail/send"), { method: "POST", body: "{nope" });
      expect(res.status).toBe(400);
    });
  });

  describe("replying", () => {
    it("replies to everyone in the thread, with Re: on the subject once, answering the latest message", async () => {
      const staged = await upload("answer.txt", "ok\n");

      const res = await post(threadPath(WIKI.threadId, "reply"), {
        body: "Both of you: thank you.",
        attachments: [staged],
      });

      expect(res.status, JSON.stringify(res.body)).toBe(200);
      const { message } = res.body as { message: MailMessage };
      expect(message).toMatchObject({
        ref: "atlas/agent/20260314_re_wiki_audit_stale_pages",
        from: { kind: "user" },
        to: ["atlas", "scout"],
        at: "2026-03-14T12:00:00Z",
        title: `Re: ${WIKI.subject}`,
        body: "Both of you: thank you.",
        in_reply_to: `atlas/agent/${WIKI.answerId}`,
        read: true,
        archived: false,
        seen_by: [
          { agent: "atlas", seen: false },
          { agent: "scout", seen: false },
        ],
      });
      expect(message.attachments).toMatchObject([
        { filename: "answer.txt", content_type: "text/plain", size: 3, agent: "atlas" },
      ]);
      for (const name of ["atlas", "scout"]) {
        const copy = agent(name).state.agentInbox.at(-1);
        expect(copy, name).toMatchObject({
          id: "20260314_re_wiki_audit_stale_pages",
          in_reply_to: `atlas/agent/${WIKI.answerId}`,
          source: "web",
        });
      }
      const result = await thread(WIKI.threadId);
      expect(result.messages).toHaveLength(5);
      expect(result.messages.at(-1)?.ref).toBe(message.ref);
      expect((await summary("sent", WIKI.threadId))?.latest_at).toBe("2026-03-14T12:00:00Z");
    });

    it("goes only to the agents of the thread, and answers an agent's message to you", async () => {
      const res = await post(threadPath(MAIL_SEED.deployTomorrow.threadIds.scout, "reply"), {
        body: "Thanks, will do.",
        attachments: [],
      });
      const { message } = res.body as { message: MailMessage };
      expect(message).toMatchObject({
        ref: "scout/agent/20260314_re_deploy_tomorrow",
        to: ["scout"],
        title: "Re: Deploy tomorrow",
        in_reply_to: "scout/user/mock_1",
      });
      expect(agent("atlas").state.agentInbox.map((item) => item.id)).not.toContain(
        "20260314_re_deploy_tomorrow",
      );
      expect(await summary("sent", MAIL_SEED.deployTomorrow.threadIds.scout)).toMatchObject({
        message_count: 2,
        in_sent: true,
        in_inbox: true,
      });
    });

    it("replies from the ref of any message of the thread", async () => {
      const res = await post(threadPath(`scout/agent/${WIKI.sentId}`, "reply"), {
        body: "Hello again.",
        attachments: [],
      });
      expect(res.status).toBe(200);
      expect((await thread(WIKI.threadId)).messages).toHaveLength(5);
    });

    it("answers 404 for a thread it doesn't have and 400 for a reply it can't send", async () => {
      expect(
        (await post(threadPath("scout/user/nothing", "reply"), { body: "Hi", attachments: [] }))
          .status,
      ).toBe(404);
      const blank = await post(threadPath(WIKI.threadId, "reply"), { body: " ", attachments: [] });
      expect(blank.status).toBe(400);
      const unknown = await post(threadPath(WIKI.threadId, "reply"), {
        body: "Hi",
        attachments: ["upload_9999"],
      });
      expect(unknown.status).toBe(400);
      expect((await post(threadPath(WIKI.threadId, "reply"), { attachments: [] })).status).toBe(
        422,
      );
      expect((await thread(WIKI.threadId)).messages).toHaveLength(4);
    });
  });

  describe("uploads", () => {
    const stage = (
      filename: string | null,
      body: Uint8Array<ArrayBuffer> | string,
    ): Promise<{ status: number; body: unknown }> =>
      fetchJson(
        url(
          `/api/hub/uploads${filename === null ? "" : `?filename=${encodeURIComponent(filename)}`}`,
        ),
        {
          method: "POST",
          body,
        },
      );

    it("stages the body and answers its id, name and size", async () => {
      const res = await stage("plan.txt", "step one\n");
      expect(res).toEqual({
        status: 200,
        body: {
          id: expect.stringMatching(/^upload_\d{4}$/) as string,
          filename: "plan.txt",
          size: 9,
        },
      });
      const { id } = res.body as { id: string };
      expect(mock.hub.hubState.uploads.get(id)?.bytes.toString()).toBe("step one\n");
    });

    it("gives each upload its own id", async () => {
      const ids = new Set<string>();
      for (let i = 0; i < 3; i++) ids.add(((await stage("a.txt", "a")).body as { id: string }).id);
      expect(ids.size).toBe(3);
    });

    it("answers 413 for a body over the limit, and takes one at the limit", async () => {
      const over = await stage("big.bin", new Uint8Array(MAX_UPLOAD_BYTES + 1));
      expect(over).toEqual({
        status: 413,
        body: { error: "The file is larger than the 50 MB upload limit." },
      });
      expect(mock.hub.hubState.uploads.size).toBe(0);
      const exact = await stage("edge.bin", new Uint8Array(MAX_UPLOAD_BYTES));
      expect(exact.status).toBe(200);
      expect((exact.body as { size: number }).size).toBe(MAX_UPLOAD_BYTES);
    });

    it("answers 400 for a missing or unusable filename and for an empty body", async () => {
      expect((await stage(null, "x")).status).toBe(400);
      for (const filename of ["", "   ", "a/b.txt", "..", "a\\b"]) {
        const res = await stage(filename, "x");
        expect(res.status, filename).toBe(400);
        expect((res.body as { error: string }).error, filename).toMatch(/^The filename /);
      }
      expect(await stage("empty.txt", "")).toEqual({
        status: 400,
        body: { error: "The upload is empty." },
      });
    });
  });
});

describe("replyTitle", () => {
  it.each([
    ["Deploy tomorrow", "Re: Deploy tomorrow"],
    ["Re: Deploy tomorrow", "Re: Deploy tomorrow"],
    ["RE: Deploy tomorrow", "RE: Deploy tomorrow"],
    ["  Deploy tomorrow  ", "Re: Deploy tomorrow"],
    ["", "Re:"],
  ])("answers %j with %j", (subject, title) => {
    expect(replyTitle(subject)).toBe(title);
  });
});
