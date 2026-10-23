import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { clearDraftAttachments, draftAttachments, readDraft, saveDraft } from "./composer-drafts";
import { notifications } from "./notifications.svelte";
import { StagedAttachment } from "./staged-attachments.svelte";
import { setViewedAgent } from "./viewed-agent";
import { ws } from "./ws.svelte";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Answers the page's reads; each upload gets the next id, `upload_1` first. */
function installServer(): { uploads: string[] } {
  const uploads: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      if (input.startsWith("/api/hub/uploads")) {
        const id = `upload_${String(uploads.length + 1)}`;
        uploads.push(id);
        return Promise.resolve(json({ id, filename: "report.pdf", size: 4 }));
      }
      if (input.includes("/chat/history")) {
        return Promise.resolve(json({ kind: "recent", messages: [], next_cursor: null }));
      }
      if (input.includes("/sessions")) {
        return Promise.resolve(json({ live: [], completed: [], next_cursor: null }));
      }
      if (input.includes("/a2a/outbound") || input.includes("/scheduled/")) {
        return Promise.resolve(json([]));
      }
      return Promise.resolve(json({}, 404));
    }),
  );
  return { uploads };
}

/** A `localStorage` that works, which the Node test environment lacks. */
function stubStorage(): void {
  const items = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => {
      items.set(key, value);
    },
    removeItem: (key: string) => {
      items.delete(key);
    },
  });
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** The id of the last message the page sent, which names the turn it starts. */
function sentMessageId(socket: FakeWebSocket): string {
  const sent = socket
    .sentFrames()
    .filter((frame) => (frame as { type?: string }).type === "send_message");
  return (sent[sent.length - 1] as { id: string }).id;
}

async function uploadedReport(): Promise<StagedAttachment> {
  const attachment = new StagedAttachment(
    new File(["%PDF"], "report.pdf", { type: "application/pdf" }),
  );
  await attachment.upload();
  return attachment;
}

function userMessages(): string[] {
  return ws.store.feed.flatMap((item) => (item.kind === "user" ? [item.content] : []));
}

let socket: FakeWebSocket;

beforeEach(async () => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
  stubStorage();
  installServer();
  setViewedAgent("scout");
  socket = FakeWebSocket.last;
  socket.simulateOpen();
  await flush();
});

afterEach(() => {
  setViewedAgent(null);
  clearDraftAttachments();
  notifications.clear();
  vi.unstubAllGlobals();
});

describe("a message the agent refused", () => {
  it("comes off the feed and goes to the listening composer, its files uploading again", async () => {
    const report = await uploadedReport();
    const refused = vi.fn();
    const stop = ws.onRefusedSend(refused);
    ws.sendChat("Read this", [report]);
    const id = sentMessageId(socket);
    expect(userMessages()).toEqual(["Read this"]);

    socket.simulateMessage({
      type: "message_refused",
      id,
      message: "An attached file isn't available anymore. Attach it again, then send.",
    });
    stop();

    expect(userMessages()).toEqual([]);
    expect(ws.store.feed.some((item) => item.kind === "turn-failure")).toBe(false);
    expect(refused).toHaveBeenCalledWith({ content: "Read this", attachments: [report] });
    expect(report.status).toBe("uploading");
    expect(report.uploadId).toBeNull();
    await report.done;
    expect(report.uploadId).toBe("upload_2");
    expect(readDraft("scout")).toBe("");
  });

  it("waits in the agent's draft when no composer is listening", async () => {
    saveDraft("scout", "typed since");
    const report = await uploadedReport();
    ws.sendChat("Read this", [report]);

    socket.simulateMessage({
      type: "message_refused",
      id: sentMessageId(socket),
      message: "Your message didn't reach the agent. Send it again.",
    });

    expect(readDraft("scout")).toBe("Read this\n\ntyped since");
    expect(draftAttachments("scout").attachments).toEqual([report]);
    await report.done;
    expect(report.status).toBe("uploaded");
    expect(report.uploadId).toBe("upload_2");
  });

  it("shows its bubble again should the agent read it after all", () => {
    ws.sendChat("Read this");
    const id = sentMessageId(socket);
    socket.simulateMessage({ type: "message_refused", id, message: "Send it again." });
    expect(userMessages()).toEqual([]);

    socket.simulateMessage({
      type: "user_message",
      id,
      turn_id: id,
      content: "Read this",
      endpoint: "ws",
    });
    expect(userMessages()).toEqual(["Read this"]);
  });
});

describe("an error naming a message the agent hasn't confirmed yet", () => {
  it("is a turn failure: the message stays, and the composer is left alone", () => {
    const refused = vi.fn();
    const stop = ws.onRefusedSend(refused);
    ws.sendChat("Hello");
    const id = sentMessageId(socket);

    // The turn's failure overtook its `user_message` and `turn_started`.
    socket.simulateMessage({ type: "error", reply_to: id, message: "The model isn't reachable." });
    stop();

    expect(refused).not.toHaveBeenCalled();
    expect(userMessages()).toEqual(["Hello"]);
    expect(ws.store.feed.some((item) => item.kind === "turn-failure")).toBe(true);
    expect(readDraft("scout")).toBe("");
  });
});
