import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  MailAttachment,
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
    it("lists the Inbox newest first, threads of one time by their ref: the agents' own messages, then the scenario's threads", async () => {
      expect(await listed("folder=inbox")).toEqual([
        MAIL_SEED.deployTomorrow.threadIds.atlas,
        MAIL_SEED.deployTomorrow.threadIds.scout,
        MAIL_SEED.dailyDigest.threadIds.atlas,
        MAIL_SEED.dailyDigest.threadIds.scout,
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
        MAIL_SEED.lastWeeksDigest.threadIds.atlas,
        MAIL_SEED.lastWeeksDigest.threadIds.scout,
      ]);
    });

    it("orders threads of one time by agent, then your messages to an agent after its messages to you, as the backend's refs sort", async () => {
      const when = mock.hub.env.clock.iso();
      const fromAgent = (name: string, id: string, inReplyTo?: string): void => {
        agent(name).state.inboxItems.push({
          id,
          title: id,
          body: id,
          source: "agent",
          timestamp: when,
          read: true,
          attachments: [],
          ...(inReplyTo === undefined ? {} : { in_reply_to: inReplyTo }),
        });
      };
      fromAgent("scout", "tie");
      fromAgent("atlas", "tie");
      // atlas answers a message of yours, which ties with the other threads on the answer's time.
      agent("atlas").state.agentInbox.push({
        id: "tie_question",
        title: "tie_question",
        body: "tie_question",
        source: "web",
        timestamp: mock.hub.env.clock.isoAgo(3_600_000),
      });
      fromAgent("atlas", "tie_answer", "atlas/agent/tie_question");

      expect((await listed("folder=inbox&limit=200")).filter((id) => id.includes("tie"))).toEqual([
        "atlas/user/tie",
        "atlas/agent/tie_question",
        "scout/user/tie",
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
        MAIL_SEED.deployTomorrow.threadIds.atlas,
        MAIL_SEED.deployTomorrow.threadIds.scout,
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
        MAIL_SEED.deployTomorrow.threadIds.scout,
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
        ["before=1790000000000:not-a-ref", "before"],
        ["agent=ghost", "ghost"],
      ] as const) {
        const res = await get(`/api/hub/mail/threads?${query}`);
        expect(res.status, query).toBe(400);
        expect((res.body as { error: string }).error, query).toContain(mentions);
      }
    });

    it("checks the folder, then the agent, the limit and the cursor, as the backend does", async () => {
      const worst = "folder=trash&agent=ghost&limit=0&before=garbage";
      const errorOf = async (query: string): Promise<string> =>
        ((await get(`/api/hub/mail/threads?${query}`)).body as { error: string }).error;

      expect(await errorOf(worst)).toContain("folder");
      expect(await errorOf("agent=ghost&limit=0&before=garbage")).toContain("ghost");
      expect(await errorOf("limit=0&before=garbage")).toContain("limit");
      expect(await errorOf("before=garbage")).toBe(
        "the before cursor isn't one this list returned",
      );
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
          url: `/api/agents/atlas/inbox/agent/${WIKI.sentId}/attachments/0`,
          agent: "atlas",
          path: `inbox/agent/attachments/${WIKI.sentId}/audit-scope.md`,
        },
      ]);
      expect(result.messages[2]?.attachments[0]).toMatchObject({
        filename: "source-notes.pdf",
        content_type: "application/pdf",
        url: `/api/agents/scout/inbox/${WIKI.replyId}/attachments/0`,
        agent: "scout",
        path: `inbox/user/attachments/${WIKI.replyId}/source-notes.pdf`,
      });
    });

    it("moves an archived message's attachments to the archive path, where the file panel still finds them, and back on restore", async () => {
      const research = MAIL_SEED.syncResearch;
      const attachment = async (): Promise<MailAttachment | undefined> =>
        (await thread(research.threadId)).messages[0]?.attachments.find(
          (candidate) => candidate.filename === "findings.md",
        );
      const open = async (found: MailAttachment): Promise<{ status: number; text: string }> => {
        const res = await fetch(
          url(`/api/agents/${found.agent}/workspace/file?path=${encodeURIComponent(found.path)}`),
        );
        return { status: res.status, text: await res.text() };
      };
      const before = await attachment();
      if (before === undefined) throw new Error("the research thread has no findings.md");
      expect(before.path).toBe(`inbox/user/attachments/${research.id}/findings.md`);
      const bytes = await (await fetch(url(before.url))).text();

      expect(await act(research.threadId, "archive")).toBe(204);

      const archived = await attachment();
      expect(archived?.path).toBe(`archive/inbox/user/attachments/${research.id}/findings.md`);
      expect(archived?.url).toBe(before.url);
      expect(await open(archived ?? before)).toEqual({ status: 200, text: bytes });
      expect((await open(before)).status).toBe(404);
      expect(await (await fetch(url(before.url))).text()).toBe(bytes);

      expect(await act(research.threadId, "restore")).toBe(204);

      expect((await attachment())?.path).toBe(before.path);
      expect(await open(before)).toEqual({ status: 200, text: bytes });
      expect(await (await fetch(url(before.url))).text()).toBe(bytes);
    });

    it("lists a thread's agents sorted by name, whichever wrote first, and replies to them all", async () => {
      // scout starts the thread and atlas answers it, so atlas sorts before the agent who came first.
      const started = await post("/api/mock/user-inbox-add?agent=scout", {
        title: "Standup notes",
        body: "Here are mine.",
      });
      const root = `scout/user/${(started.body as { id: string }).id}`;
      await post("/api/mock/user-inbox-add?agent=atlas", {
        body: "And mine.",
        in_reply_to: root,
      });

      const result = await thread(root);
      expect(result.messages.map((message) => message.from)).toEqual([
        { kind: "agent", agent: "scout" },
        { kind: "agent", agent: "atlas" },
      ]);
      expect(result.participants).toEqual(["atlas", "scout"]);
      expect(result.recipients_for_reply).toEqual(["atlas", "scout"]);
      expect((await summary("inbox", root))?.participants).toEqual(["atlas", "scout"]);
      const reply = await post(threadPath(root, "reply"), { body: "Thanks.", attachments: [] });
      expect((reply.body as { message: MailMessage }).message.to).toEqual(["atlas", "scout"]);
    });

    it("takes the subject from the root and the latest message by time, even when a reply carries an earlier time", async () => {
      const rootId = "backdated_root";
      agent("scout").state.inboxItems.push({
        id: rootId,
        title: "Backdated",
        body: "The root.",
        source: "agent",
        timestamp: mock.hub.env.clock.iso(),
        read: true,
        attachments: [],
      });
      agent("atlas").state.inboxItems.push({
        id: "backdated_reply",
        title: "Re: Backdated",
        body: "Written before the root.",
        source: "agent",
        timestamp: mock.hub.env.clock.isoAgo(3_600_000),
        read: true,
        attachments: [],
        in_reply_to: `scout/user/${rootId}`,
      });

      const result = await thread(`scout/user/${rootId}`);
      expect(result.subject).toBe("Backdated");
      expect(result.messages.map((message) => message.ref)).toEqual([
        `scout/user/${rootId}`,
        "atlas/user/backdated_reply",
      ]);
      expect(await summary("inbox", `scout/user/${rootId}`)).toMatchObject({
        subject: "Backdated",
        snippet: "The root.",
        latest_at: "2026-03-14T12:00:00Z",
      });
      const reply = await post(threadPath(`scout/user/${rootId}`, "reply"), {
        body: "Got both.",
        attachments: [],
      });
      expect((reply.body as { message: MailMessage }).message).toMatchObject({
        title: "Re: Backdated",
        in_reply_to: `scout/user/${rootId}`,
      });
    });

    it("keeps your message and an agent's message with one id apart: each has its own attachment route", async () => {
      const sent = await post("/api/hub/mail/send", {
        to: ["atlas"],
        subject: "Pairing",
        body: "Free tomorrow?",
        attachments: [],
      });
      const { thread_id } = sent.body as { thread_id: string };
      await post("/api/mock/user-inbox-add?agent=atlas", {
        body: "Yes.",
        in_reply_to: thread_id,
        attachments: [{ filename: "theirs.txt" }],
      });
      const staged = await upload("mine.txt", "mine\n");
      await post(threadPath(thread_id, "reply"), { body: "Great.", attachments: [staged] });

      const [, theirs, mine] = (await thread(thread_id)).messages;
      // Both messages are `Re: Pairing` from the same day, so they share an id.
      expect(theirs?.ref).toBe("atlas/user/20260314_re_pairing");
      expect(mine?.ref).toBe("atlas/agent/20260314_re_pairing");
      expect(theirs?.attachments[0]?.url).toBe(
        "/api/agents/atlas/inbox/20260314_re_pairing/attachments/0",
      );
      expect(mine?.attachments[0]?.url).toBe(
        "/api/agents/atlas/inbox/agent/20260314_re_pairing/attachments/0",
      );

      const serve = async (attachmentUrl: string | undefined): Promise<string> =>
        (await fetch(url(attachmentUrl ?? ""))).text();
      expect(await serve(theirs?.attachments[0]?.url)).toBe("Mock attachment: theirs.txt\n");
      expect(await serve(mine?.attachments[0]?.url)).toBe("mine\n");

      // Both routes are file routes, so they answer with the agent stopped.
      await post("/api/hub/agents/atlas/stop");
      expect(await serve(theirs?.attachments[0]?.url)).toBe("Mock attachment: theirs.txt\n");
      expect(await serve(mine?.attachments[0]?.url)).toBe("mine\n");
    });

    it("keeps each route to its own inbox: an id on the wrong side is not found", async () => {
      const staged = await upload("only-mine.txt", "mine\n");
      await post("/api/hub/mail/send", {
        to: ["atlas"],
        subject: "Solo",
        body: "No answer.",
        attachments: [staged],
      });
      const id = "20260314_solo";
      expect((await fetch(url(`/api/agents/atlas/inbox/agent/${id}/attachments/0`))).status).toBe(
        200,
      );
      const wrongSide = await fetchJson(url(`/api/agents/atlas/inbox/${id}/attachments/0`));
      expect(wrongSide).toEqual({
        status: 404,
        body: {
          error:
            "This attachment is no longer available. It may have been removed from its inbox item.",
        },
      });
    });

    it("answers 404 for a message it doesn't have, naming it, and 400 for a path that isn't UTF-8", async () => {
      expect(await get(threadPath("scout/user/nothing"))).toEqual({
        status: 404,
        body: { error: "There is no message 'scout/user/nothing'. It may have been deleted." },
      });
      for (const action of ["read", "archive", "restore"] as const) {
        expect(await act("scout/user/nothing", action), action).toBe(404);
      }
      // Anything that isn't a ref names no message, including an escape that isn't one.
      expect(await get("/api/hub/mail/threads/%ZZ")).toEqual({
        status: 404,
        body: { error: "There is no message '%ZZ'. It may have been deleted." },
      });
      expect((await get(threadPath("not a ref"))).status).toBe(404);
      // The router refuses bytes that aren't UTF-8 before the route runs.
      expect((await fetch(url("/api/hub/mail/threads/%E0%A4%A"))).status).toBe(400);
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

      const asText = await fetch(url(`/api/agents/scout/inbox/agent/${id}/attachments/0`));
      expect(asText.status).toBe(200);
      expect(asText.headers.get("content-type")).toBe("text/markdown");
      expect(await asText.text()).toBe("# Notes\n\nA list.\n");
      const asBinary = await fetch(url(`/api/agents/scout/inbox/agent/${id}/attachments/1`));
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
        const res = await fetch(
          url(`/api/agents/${name}/inbox/agent/20260314_shared/attachments/0`),
        );
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

    it("sends to a stopped agent, titling a message with no subject by the first line of its body", async () => {
      for (const subject of [{ subject: "" }, { subject: "  " }, {}]) {
        const sent = await post("/api/hub/mail/send", {
          to: ["drifter"],
          ...subject,
          body: "Wake up.\nIt is late.",
          attachments: [],
        });
        expect(sent.status, JSON.stringify(subject)).toBe(200);
      }
      const ids = (await listed("folder=sent&agent=drifter")).filter((id) =>
        id.includes("wake_up"),
      );
      expect(ids.sort()).toEqual([
        "drifter/agent/20260314_wake_up",
        "drifter/agent/20260314_wake_up_2",
        "drifter/agent/20260314_wake_up_3",
      ]);
      expect(await summary("sent", "drifter/agent/20260314_wake_up")).toMatchObject({
        subject: "Wake up.",
        snippet: "Wake up. It is late.",
      });
    });

    it("shows a note added with /inbox in Sent, as a message from you to that agent", async () => {
      const socket = await mock.openSocket("/api/agents/atlas/ws");
      socket.send({ type: "inbox_add", body: "remember the milk\nand the eggs" });
      expect(await socket.nextOfType("notice")).toEqual({
        type: "notice",
        message: "Added a note to the inbox.",
      });

      const id = "atlas/agent/20260314_remember_the_milk";
      expect(await summary("sent", id)).toMatchObject({
        subject: "remember the milk",
        participants: ["atlas"],
        message_count: 1,
        snippet: "remember the milk and the eggs",
        in_sent: true,
        in_inbox: false,
        archived: false,
      });
      const [message] = (await thread(id)).messages;
      expect(message).toMatchObject({
        ref: id,
        from: { kind: "user" },
        to: ["atlas"],
        title: "remember the milk",
        body: "remember the milk\nand the eggs",
        seen_by: [{ agent: "atlas", seen: false }],
      });
      expect(await listed("folder=sent&agent=scout")).not.toContain(id);

      socket.send({ type: "inbox_add", body: "remember the milk" });
      await socket.nextOfType("notice");
      expect(
        agent("atlas")
          .state.agentInbox.filter((item) => item.title === "remember the milk")
          .map((item) => item.id),
      ).toEqual(["20260314_remember_the_milk", "20260314_remember_the_milk_2"]);
    });

    it("keeps a subject as it is written", async () => {
      const sent = await post("/api/hub/mail/send", {
        to: ["scout"],
        subject: "  Spaced  ",
        body: "Hi",
        attachments: [],
      });
      const { thread_id } = sent.body as { thread_id: string };
      expect((await thread(thread_id)).subject).toBe("  Spaced  ");
    });

    it("answers 400 for a message it can't send, naming why", async () => {
      const staged = await upload("a.txt", "a");
      const cases: [unknown, string][] = [
        [{ to: ["scout"], subject: "Hi", body: "   \n", attachments: [] }, "Write a message"],
        [{ to: [], subject: "Hi", body: "Hi", attachments: [] }, "at least one agent"],
        [{ to: ["scout", "ghost"], subject: "Hi", body: "Hi", attachments: [] }, "'ghost'"],
        [
          { to: ["scout"], subject: "Hi", body: "Hi", attachments: ["upload_9999"] },
          "(upload 'upload_9999') isn't available anymore",
        ],
        // The staged files are checked first, then the recipients, then the words.
        [{ to: [], subject: "", body: " ", attachments: ["upload_9999"] }, "upload_9999"],
        [{ to: [], subject: "", body: " ", attachments: [] }, "at least one agent"],
        [{ to: ["ghost"], subject: "", body: " ", attachments: [] }, "Write a message"],
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

    it("answers 400 for a body it can't read, and sends one that leaves out the subject and attachments", async () => {
      for (const body of [
        {},
        { to: "scout", subject: "", body: "x" },
        { to: ["scout"], subject: 1, body: "x" },
        { to: ["scout"], subject: null, body: "x" },
        { to: ["scout"], subject: "", body: "x", attachments: [1] },
        { to: ["scout"], subject: "" },
      ]) {
        const res = await post("/api/hub/mail/send", body);
        expect(res.status, JSON.stringify(body)).toBe(400);
        expect((res.body as { error: string }).error).toMatch(/^The message couldn't be read: /);
      }
      const res = await fetchJson(url("/api/hub/mail/send"), { method: "POST", body: "{nope" });
      expect(res.status).toBe(400);
      expect((res.body as { error: string }).error).toMatch(/^The message couldn't be read: /);

      const bare = await post("/api/hub/mail/send", { to: ["scout"], body: "Just this." });
      expect(bare.status, JSON.stringify(bare.body)).toBe(200);
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
      const unreadable = await post(threadPath(WIKI.threadId, "reply"), { attachments: [] });
      expect(unreadable.status).toBe(400);
      expect((unreadable.body as { error: string }).error).toMatch(
        /^The message couldn't be read: .*`body`/,
      );
      expect((await thread(WIKI.threadId)).messages).toHaveLength(4);
    });

    it("looks the thread up before it reads the body, as the backend does", async () => {
      const ref = "scout/user/nothing";
      for (const body of [{}, { body: 7 }, { body: " ", attachments: ["upload_9999"] }]) {
        const res = await post(threadPath(ref, "reply"), body);
        expect(res.status, JSON.stringify(body)).toBe(404);
        expect(res.body).toEqual({
          error: `There is no message '${ref}'. It may have been deleted.`,
        });
      }
      const unreadable = await fetchJson(url(threadPath(ref, "reply")), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{not json",
      });
      expect(unreadable.status).toBe(404);
    });

    it("sends a reply that leaves out the attachments", async () => {
      const res = await post(threadPath(WIKI.threadId, "reply"), { body: "No files." });
      expect(res.status, JSON.stringify(res.body)).toBe(200);
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
    ["RE: Deploy tomorrow", "Re: Deploy tomorrow"],
    ["RE: re:Deploy tomorrow", "Re: Deploy tomorrow"],
    ["  Deploy tomorrow  ", "Re: Deploy tomorrow"],
    ["Reply needed", "Re: Reply needed"],
    ["Re:", "Re:"],
    ["", "Re:"],
  ])("answers %j with %j", (subject, title) => {
    expect(replyTitle(subject)).toBe(title);
  });
});
