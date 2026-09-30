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
  agent: string;
  address: string;
  on(type: string, handler: (frame: Frame) => void): () => void;
  send(text: string): Promise<string>;
  stop(): Promise<void>;
}

interface Sdk {
  state: { get(): Promise<unknown>; set(value: unknown): Promise<void> };
  sessions: {
    start(options: { agent?: string; prompt: string; model?: string }): Promise<SessionHandle>;
  };
  ask(request: string | Record<string, unknown>, options?: { agent?: string }): Promise<unknown>;
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
    const started = sdk.sessions.start({ agent: "scout", prompt: "write a page", model: "small" });
    await settle();

    expect(posted.some((m) => m.kind === "subscribe")).toBe(true);
    const request = lastFetch(posted);
    expect(request.method).toBe("POST");
    expect(request.path).toBe("/api/agents/scout/sessions");
    expect(JSON.parse(request.body ?? "")).toEqual({ prompt: "write a page", model: "small" });

    reply(request.id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;
    expect(handle.address).toBe("artifact-wiki-0001");
    expect(handle.agent).toBe("scout");
  });

  it("needs an agent, and asks nothing of the gateway without one", async () => {
    const { sdk, posted } = loadSdk();
    await expect(sdk.sessions.start({ prompt: "go" })).rejects.toThrow(
      "residuum.sessions.start needs { agent, prompt }",
    );
    await expect(sdk.sessions.start({ agent: "", prompt: "go" })).rejects.toThrow("needs");
    expect(posted.filter((m) => m.kind === "fetch")).toEqual([]);
  });

  it("escapes the agent name in the request path", async () => {
    const { sdk, posted } = loadSdk();
    void sdk.sessions.start({ agent: "a/b", prompt: "go" }).catch(() => undefined);
    await settle();
    expect(lastFetch(posted).path).toBe("/api/agents/a%2Fb/sessions");
  });

  it("rejects with the state when the agent isn't running", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "quiet", prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 409, { error: "quiet is stopped", state: "stopped" });
    await expect(started).rejects.toMatchObject({
      message: "quiet is stopped",
      state: "stopped",
      status: 409,
    });
  });

  it("rejects with the gateway's error when the agent doesn't exist", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "ghost", prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 404, { error: "no agent named 'ghost'" });
    await expect(started).rejects.toMatchObject({
      message: "no agent named 'ghost'",
      status: 404,
    });
  });

  it("delivers only its own session's frames, including ones that beat the reply", async () => {
    const { sdk, posted, deliver, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    // The run can announce itself before the start request's reply arrives.
    deliver({
      kind: "event",
      frame: {
        agent: "scout",
        type: "session_started",
        session: { address: "artifact-wiki-0001", run_id: "r1" },
      },
    });
    deliver({
      kind: "event",
      frame: {
        agent: "scout",
        type: "session_started",
        session: { address: "artifact-other-0002", run_id: "r2" },
      },
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
      frame: {
        agent: "scout",
        type: "session_response",
        address: "artifact-other-0002",
        content: "not mine",
      },
    });
    deliver({
      kind: "event",
      frame: {
        agent: "scout",
        type: "session_response",
        address: "artifact-wiki-0001",
        content: "mine",
      },
    });
    deliver({ kind: "event", frame: { type: "chat_message", content: "main chat" } });
    deliver({
      kind: "event",
      frame: { agent: "scout", type: "session_message_delivered", address: "artifact-wiki-0001" },
    });
    await settle();

    expect(all.map((f) => f.type)).toEqual(["session_started", "session_response"]);
    expect(all.map((f) => f.content ?? f.session?.run_id)).toEqual(["r1", "mine"]);
  });

  it("keys sessions by agent and address, so a same-address session on another agent is never mixed in", async () => {
    const { sdk, posted, deliver, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    // Another agent's session can hold the very same address.
    deliver({
      kind: "event",
      frame: {
        agent: "atlas",
        type: "session_started",
        session: { address: "artifact-wiki-0001", run_id: "atlas-run" },
      },
    });
    reply(lastFetch(posted).id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;
    const all: Frame[] = [];
    handle.on("*", (frame) => all.push(frame));
    await settle();
    expect(all).toEqual([]);

    deliver({
      kind: "event",
      frame: {
        agent: "atlas",
        type: "session_response",
        address: "artifact-wiki-0001",
        content: "atlas",
      },
    });
    deliver({
      kind: "event",
      frame: {
        agent: "scout",
        type: "session_response",
        address: "artifact-wiki-0001",
        content: "scout",
      },
    });
    // A session frame that names no agent belongs to no handle.
    deliver({
      kind: "event",
      frame: { type: "session_response", address: "artifact-wiki-0001", content: "unattributed" },
    });
    await settle();
    expect(all.map((f) => f.content)).toEqual(["scout"]);
  });

  it("sends messages and stops through the session's own endpoints", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 202, { address: "artifact-wiki-0001" });
    const handle = await started;

    const sent = handle.send("keep going");
    await settle();
    const message = lastFetch(posted);
    expect(message.path).toBe("/api/agents/scout/sessions/artifact-wiki-0001/messages");
    expect(JSON.parse(message.body ?? "")).toEqual({ content: "keep going" });
    reply(message.id ?? "", 200, { outcome: "live" });
    expect(await sent).toBe("live");

    const stopped = handle.stop();
    await settle();
    const stop = lastFetch(posted);
    expect(stop.path).toBe("/api/agents/scout/sessions/artifact-wiki-0001/stop");
    reply(stop.id ?? "", 404, { error: "not running", code: "not_live" });
    await expect(stopped).rejects.toMatchObject({ message: "not running", code: "not_live" });
  });

  it("rejects with the gateway's error when the start is refused", async () => {
    const { sdk, posted, reply } = loadSdk();
    const started = sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    reply(lastFetch(posted).id ?? "", 400, { error: "unknown skill" });
    await expect(started).rejects.toThrow("unknown skill");
  });
});

describe("residuum.state", () => {
  it("reads the artifact's state file from the team workbench", async () => {
    const { sdk, posted, reply } = loadSdk();
    const loaded = sdk.state.get();
    await settle();

    const request = lastFetch(posted);
    expect(request.method).toBe("GET");
    expect(request.path).toBe("/api/team/workspace/file?path=workbench%2Fwiki.state.json");

    reply(request.id ?? "", 200, { picked: 3 });
    expect(await loaded).toEqual({ picked: 3 });
  });

  it("resolves to null before the first set", async () => {
    const { sdk, posted, reply } = loadSdk();
    const loaded = sdk.state.get();
    await settle();
    reply(lastFetch(posted).id ?? "", 404, "");
    expect(await loaded).toBeNull();
  });

  it("writes the state file into the team workbench", async () => {
    const { sdk, posted, reply } = loadSdk();
    const saved = sdk.state.set({ picked: 4 });
    await settle();

    const request = lastFetch(posted);
    expect(request.method).toBe("PUT");
    expect(request.path).toBe("/api/team/workspace/file");
    expect(JSON.parse(request.body ?? "")).toEqual({
      path: "workbench/wiki.state.json",
      content: JSON.stringify({ picked: 4 }),
    });

    reply(request.id ?? "", 200, {});
    await saved;
  });
});

describe("residuum.ask", () => {
  it("calls the named agent's model route and leaves the agent out of the body", async () => {
    const { sdk, posted, reply } = loadSdk();
    const asked = sdk.ask({ agent: "scout", prompt: "summarize", max_tokens: 50 });
    await settle();

    const request = lastFetch(posted);
    expect(request.method).toBe("POST");
    expect(request.path).toBe("/api/agents/scout/model/complete");
    expect(JSON.parse(request.body ?? "")).toEqual({ prompt: "summarize", max_tokens: 50 });

    reply(request.id ?? "", 200, { text: "done" });
    expect(await asked).toEqual({ text: "done" });
  });

  it("takes the agent as a second argument for the prompt shorthand", async () => {
    const { sdk, posted } = loadSdk();
    void sdk.ask("hello", { agent: "scout" });
    await settle();
    const request = lastFetch(posted);
    expect(request.path).toBe("/api/agents/scout/model/complete");
    expect(JSON.parse(request.body ?? "")).toEqual({ prompt: "hello" });
  });

  it("needs an agent, and asks nothing of the gateway without one", async () => {
    const { sdk, posted } = loadSdk();
    await expect(sdk.ask("hello")).rejects.toThrow("residuum.ask needs an agent");
    await expect(sdk.ask({ prompt: "hello" })).rejects.toThrow("needs");
    expect(posted.filter((m) => m.kind === "fetch")).toEqual([]);
  });
});
