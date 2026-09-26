import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { BRIDGE_TAG } from "./workbench-bridge";

// Runs the real SDK (assets/workbench/sdk.js) against a stand-in for the
// artifact's window and the bridge on the other side of postMessage.

const SDK_SOURCE = readFileSync(
  new URL("../../../assets/workbench/sdk.js", import.meta.url),
  "utf8",
);

interface Frame {
  type: string;
  address?: string;
  session?: { address: string; run_id: string };
  content?: string;
}

interface SessionHandle {
  address: string;
  on(type: string, handler: (frame: Frame) => void): () => void;
  send(text: string): Promise<string>;
  stop(): Promise<void>;
}

interface Sdk {
  sessions: { start(options: { prompt: string; model?: string }): Promise<SessionHandle> };
}

interface Posted {
  kind: string;
  id?: string;
  path?: string;
  method?: string;
  body?: string | null;
}

type Listener = (event: { source: unknown; data: unknown }) => void;

/** Load the SDK into a fake embedded window; returns it and the bridge side. */
function loadSdk(): {
  sdk: Sdk;
  posted: Posted[];
  deliver: (data: Record<string, unknown>) => void;
  reply: (id: string, status: number, body: unknown) => void;
} {
  const posted: Posted[] = [];
  const listeners: Listener[] = [];
  const parent = {
    postMessage: (message: Posted): void => {
      posted.push(message);
    },
  };
  const win: Record<string, unknown> = {
    parent,
    addEventListener: (type: string, listener: Listener): void => {
      if (type === "message") listeners.push(listener);
    },
  };
  runInNewContext(SDK_SOURCE, {
    window: win,
    __RESIDUUM_ARTIFACT__: "wiki",
    __RESIDUUM_VERSION__: "2026.09.23",
    __RESIDUUM_FEATURES__: ["artifact-sessions"],
    Response,
    queueMicrotask,
    console,
  });
  const deliver = (data: Record<string, unknown>): void => {
    for (const listener of listeners)
      listener({ source: parent, data: { tag: BRIDGE_TAG, ...data } });
  };
  const reply = (id: string, status: number, body: unknown): void => {
    const bytes = new TextEncoder().encode(JSON.stringify(body));
    deliver({
      kind: "result",
      id,
      result: {
        status,
        statusText: "",
        headers: [["content-type", "application/json"]],
        body: bytes.buffer,
      },
    });
  };
  return { sdk: win.residuum as Sdk, posted, deliver, reply };
}

function lastFetch(posted: Posted[]): Posted {
  const fetches = posted.filter((m) => m.kind === "fetch");
  const last = fetches[fetches.length - 1];
  if (!last) throw new Error("no fetch was posted");
  return last;
}

/** Let the SDK's promise chains and queued microtasks run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

describe("residuum.sessions.start", () => {
  it("posts the start request and hands back a handle for the new address", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ prompt: "write a page", model: "small" });
    await settle();

    expect(posted.some((m) => m.kind === "subscribe")).toBe(true);
    const request = lastFetch(posted);
    expect(request.method).toBe("POST");
    expect(request.path).toBe("/api/sessions");
    expect(JSON.parse(request.body ?? "")).toEqual({ prompt: "write a page", model: "small" });

    reply(request.id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;
    expect(handle.address).toBe("artifact-wiki-0001");
  });

  it("delivers only its own session's frames, including ones that beat the reply", async () => {
    const { sdk, posted, deliver, reply } = loadSdk();
    const started = sdk.sessions.start({ prompt: "go" });
    await settle();
    // The run can announce itself before the start request's reply arrives.
    deliver({
      kind: "event",
      frame: { type: "session_started", session: { address: "artifact-wiki-0001", run_id: "r1" } },
    });
    deliver({
      kind: "event",
      frame: { type: "session_started", session: { address: "artifact-other-0002", run_id: "r2" } },
    });
    reply(lastFetch(posted).id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;

    const announced: Frame[] = [];
    const all: Frame[] = [];
    handle.on("session_started", (frame) => announced.push(frame));
    handle.on("*", (frame) => all.push(frame));
    await settle();
    expect(announced.map((f) => f.session?.run_id)).toEqual(["r1"]);

    deliver({
      kind: "event",
      frame: { type: "session_response", address: "artifact-other-0002", content: "not mine" },
    });
    deliver({
      kind: "event",
      frame: { type: "session_response", address: "artifact-wiki-0001", content: "mine" },
    });
    deliver({ kind: "event", frame: { type: "chat_message", content: "main chat" } });
    deliver({
      kind: "event",
      frame: { type: "session_message_delivered", address: "artifact-wiki-0001" },
    });
    await settle();

    expect(all.map((f) => f.type)).toEqual(["session_started", "session_response"]);
    expect(all.map((f) => f.content ?? f.session?.run_id)).toEqual(["r1", "mine"]);
  });

  it("sends messages and stops through the session's own endpoints", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;

    const sent = handle.send("keep going");
    await settle();
    const message = lastFetch(posted);
    expect(message.path).toBe("/api/sessions/artifact-wiki-0001/messages");
    expect(JSON.parse(message.body ?? "")).toEqual({ content: "keep going" });
    reply(message.id ?? "", 200, { outcome: "live" });
    expect(await sent).toBe("live");

    const stopped = handle.stop();
    await settle();
    const stop = lastFetch(posted);
    expect(stop.path).toBe("/api/sessions/artifact-wiki-0001/stop");
    reply(stop.id ?? "", 404, { error: "not running", code: "not_live" });
    await expect(stopped).rejects.toMatchObject({ message: "not running", code: "not_live" });
  });

  it("rejects with the gateway's error when the start is refused", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 400, { error: "unknown skill" });
    await expect(started).rejects.toThrow("unknown skill");
  });
});
