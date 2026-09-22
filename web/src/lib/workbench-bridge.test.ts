import { describe, expect, it, vi } from "vitest";
import {
  BRIDGE_TAG,
  checkToolRequest,
  parseToolRequest,
  WorkbenchBridge,
  type BridgeDeps,
  type FrameTarget,
  type RelayedResponse,
} from "./workbench-bridge";
import type { ServerMessage } from "./types";

const ORIGIN = "https://bear.agent-residuum.com";

describe("checkToolRequest", () => {
  it.each([
    ["GET", "/api/status"],
    ["GET", "/api/workspace/file?path=workbench/chart.state.json"],
    ["PUT", "/api/workspace/file"],
    ["GET", "/api/secrets"],
    ["GET", "/api/agent-keys"],
    ["GET", "/api/workbench/tools"],
    ["GET", "/api/tracing/status"],
    ["POST", "/api/inbox/abc/archive"],
  ])("allows %s %s", (method, path) => {
    expect(checkToolRequest(method, path, ORIGIN)).toEqual({ allowed: true, url: path });
  });

  it.each([
    ["POST", "/api/secrets"],
    ["DELETE", "/api/secrets/openai"],
    ["POST", "/api/agent-keys"],
    ["GET", "/api/config/raw"],
    ["PUT", "/api/config/raw"],
    ["GET", "/api/providers/raw"],
    ["PUT", "/api/mcp/raw"],
    ["POST", "/api/config/complete-setup"],
    ["POST", "/api/shutdown"],
    ["POST", "/api/shutdown/"],
    ["POST", "/api/update/apply"],
    ["POST", "/api/update/restart"],
    ["POST", "/api/cloud/disconnect"],
    ["POST", "/api/tracing/sanitize"],
    ["POST", "/api/tracing/otel/endpoints"],
    ["DELETE", "/api/workbench/tools/chart"],
  ])("blocks %s %s", (method, path) => {
    const check = checkToolRequest(method, path, ORIGIN);
    expect(check.allowed).toBe(false);
    if (!check.allowed) expect(check.status).toBe(403);
  });

  it.each([
    "/api/workspace/../secrets",
    "/api/%2e%2e/api/secrets",
    "/api/workspace/%2E%2E/secrets",
    "/api/secrets%2Fopenai",
  ])("blocks secret writes reached through path tricks: %s", (path) => {
    expect(checkToolRequest("POST", path, ORIGIN).allowed).toBe(false);
  });

  it.each([
    "https://evil.example/api/status",
    "//evil.example/api/status",
    "/ws",
    "/webhook/deploy",
    "/",
    "status",
  ])("refuses anything outside the gateway API: %s", (path) => {
    expect(checkToolRequest("GET", path, ORIGIN).allowed).toBe(false);
  });

  it("refuses unusual methods", () => {
    expect(checkToolRequest("TRACE", "/api/status", ORIGIN).allowed).toBe(false);
  });
});

describe("parseToolRequest", () => {
  it("accepts a well-formed fetch", () => {
    expect(
      parseToolRequest({
        tag: BRIDGE_TAG,
        kind: "fetch",
        id: "req-1",
        path: "/api/status",
        method: "GET",
        headers: {},
        body: null,
      }),
    ).toMatchObject({ kind: "fetch", id: "req-1" });
  });

  it.each([
    null,
    "string",
    { kind: "subscribe" },
    { tag: "other", kind: "subscribe" },
    {
      tag: BRIDGE_TAG,
      kind: "fetch",
      id: "x",
      path: "/api",
      method: "GET",
      headers: { a: 1 },
      body: null,
    },
    { tag: BRIDGE_TAG, kind: "send", id: 3, content: "hi" },
    { tag: BRIDGE_TAG, kind: "eval" },
  ])("rejects %j", (data) => {
    expect(parseToolRequest(data)).toBeNull();
  });
});

interface Harness {
  bridge: WorkbenchBridge;
  frame: FrameTarget & { posted: Record<string, unknown>[] };
  deps: BridgeDeps;
  emit: (msg: ServerMessage) => void;
  sent: string[];
}

function harness(overrides: Partial<BridgeDeps> = {}): Harness {
  const posted: Record<string, unknown>[] = [];
  const frame = {
    posted,
    postMessage: (message: unknown) => posted.push(message as Record<string, unknown>),
  };
  let listener: ((msg: ServerMessage) => void) | null = null;
  const sent: string[] = [];
  const deps: BridgeDeps = {
    origin: ORIGIN,
    fetch: vi.fn(() => Promise.resolve(new Response('{"ok":true}', { status: 200 }))),
    hasUserActivation: () => true,
    isConnected: () => true,
    sendToAgent: (content) => sent.push(content),
    onFrame: (l) => {
      listener = l;
      return () => {
        listener = null;
      };
    },
    ...overrides,
  };
  const bridge = new WorkbenchBridge("chart", () => frame, deps);
  bridge.start();
  return { bridge, frame, deps, emit: (msg) => listener?.(msg), sent };
}

const fetchMsg = (path: string, method = "GET"): Record<string, unknown> => ({
  tag: BRIDGE_TAG,
  kind: "fetch",
  id: "req-1",
  path,
  method,
  headers: {},
  body: null,
});

describe("WorkbenchBridge", () => {
  it("ignores messages from anything but the tool's frame", async () => {
    const h = harness();
    await h.bridge.handleMessage({}, fetchMsg("/api/status"));
    expect(h.deps.fetch).not.toHaveBeenCalled();
    expect(h.frame.posted).toEqual([]);
  });

  it("relays an allowed request and its response", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, fetchMsg("/api/status"));
    expect(h.deps.fetch).toHaveBeenCalledWith(
      "/api/status",
      expect.objectContaining({ method: "GET" }),
    );
    const reply = h.frame.posted[0];
    expect(reply).toMatchObject({ tag: BRIDGE_TAG, kind: "result", id: "req-1" });
    const result = reply?.result as RelayedResponse;
    expect(result.status).toBe(200);
    expect(new TextDecoder().decode(result.body)).toBe('{"ok":true}');
  });

  it("answers a blocked request with a 403 without calling the gateway", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, fetchMsg("/api/shutdown", "POST"));
    expect(h.deps.fetch).not.toHaveBeenCalled();
    const result = h.frame.posted[0]?.result as RelayedResponse;
    expect(result.status).toBe(403);
    expect(JSON.parse(new TextDecoder().decode(result.body))).toHaveProperty("error");
  });

  it("reports a network failure as an error", async () => {
    const h = harness({ fetch: vi.fn(() => Promise.reject(new TypeError("offline"))) });
    await h.bridge.handleMessage(h.frame, fetchMsg("/api/status"));
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("sends to the agent after a user gesture, labelled with the tool", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, {
      tag: BRIDGE_TAG,
      kind: "send",
      id: "s1",
      content: " pick B ",
    });
    expect(h.sent).toEqual(['[From workbench tool "chart"]\npick B']);
    expect(h.frame.posted[0]).toMatchObject({ id: "s1", result: null });
  });

  it("refuses to message the agent without a user gesture", async () => {
    const h = harness({ hasUserActivation: () => false });
    await h.bridge.handleMessage(h.frame, {
      tag: BRIDGE_TAG,
      kind: "send",
      id: "s1",
      content: "hi",
    });
    expect(h.sent).toEqual([]);
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("refuses to message the agent while disconnected", async () => {
    const h = harness({ isConnected: () => false });
    await h.bridge.handleMessage(h.frame, {
      tag: BRIDGE_TAG,
      kind: "send",
      id: "s1",
      content: "hi",
    });
    expect(h.sent).toEqual([]);
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("forwards server frames only after the tool subscribes, until its document changes", async () => {
    const h = harness();
    const frame: ServerMessage = { type: "workbench_tool_updated", name: "chart" };
    h.emit(frame);
    expect(h.frame.posted).toEqual([]);

    await h.bridge.handleMessage(h.frame, { tag: BRIDGE_TAG, kind: "subscribe" });
    h.emit(frame);
    expect(h.frame.posted).toEqual([{ tag: BRIDGE_TAG, kind: "event", frame }]);

    h.bridge.documentChanged();
    h.emit(frame);
    expect(h.frame.posted).toHaveLength(1);
  });
});
