import { describe, expect, it, vi } from "vitest";
import {
  BRIDGE_TAG,
  checkArtifactRequest,
  parseArtifactRequest,
  WorkbenchBridge,
  type BridgeDeps,
  type FrameTarget,
  type RelayedResponse,
} from "./workbench-bridge";
import type { ServerMessage } from "./types";

const ORIGIN = "https://bear.agent-residuum.com";
const ARTIFACTS = "https://bear.workbench.agent-residuum.com";

describe("checkArtifactRequest", () => {
  it.each([
    ["GET", "/api/status"],
    ["GET", "/api/workspace/file?path=workbench/chart.state.json"],
    ["PUT", "/api/workspace/file"],
    ["GET", "/api/secrets"],
    ["GET", "/api/agent-keys"],
    ["GET", "/api/workbench/artifacts"],
    ["GET", "/api/tracing/status"],
    ["POST", "/api/inbox/abc/archive"],
    ["PUT", "/api/mcp/raw"],
    ["DELETE", "/api/workbench/artifacts/chart"],
  ])("allows %s %s", (method, path) => {
    expect(checkArtifactRequest(method, path, ORIGIN)).toEqual({ allowed: true, url: path });
  });

  it.each([
    ["POST", "/api/secrets"],
    ["DELETE", "/api/secrets/openai"],
    ["POST", "/api/agent-keys"],
    ["GET", "/api/config/raw"],
    ["PUT", "/api/config/raw"],
    ["GET", "/api/providers/raw"],
    ["POST", "/api/config/complete-setup"],
    ["POST", "/api/shutdown"],
    ["POST", "/api/shutdown/"],
    ["POST", "/api/update/apply"],
    ["POST", "/api/update/restart"],
    ["POST", "/api/cloud/disconnect"],
    ["POST", "/api/tracing/sanitize"],
    ["POST", "/api/tracing/otel/endpoints"],
  ])("blocks %s %s", (method, path) => {
    const check = checkArtifactRequest(method, path, ORIGIN);
    expect(check.allowed).toBe(false);
    if (!check.allowed) expect(check.status).toBe(403);
  });

  it.each([
    "/api/workspace/../secrets",
    "/api/%2e%2e/api/secrets",
    "/api/workspace/%2E%2E/secrets",
    "/api/secrets%2Fopenai",
  ])("blocks secret writes reached through path tricks: %s", (path) => {
    expect(checkArtifactRequest("POST", path, ORIGIN).allowed).toBe(false);
  });

  it.each([
    "https://evil.example/api/status",
    "//evil.example/api/status",
    "/ws",
    "/webhook/deploy",
    "/",
    "status",
  ])("refuses anything outside the gateway API: %s", (path) => {
    expect(checkArtifactRequest("GET", path, ORIGIN).allowed).toBe(false);
  });

  it("refuses unusual methods", () => {
    expect(checkArtifactRequest("TRACE", "/api/status", ORIGIN).allowed).toBe(false);
  });
});

describe("parseArtifactRequest", () => {
  it("accepts a well-formed fetch", () => {
    expect(
      parseArtifactRequest({
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

  it("accepts an ArrayBuffer body", () => {
    const body = new Uint8Array([1, 2, 3]).buffer;
    expect(
      parseArtifactRequest({
        tag: BRIDGE_TAG,
        kind: "fetch",
        id: "req-1",
        path: "/api/workspace/raw",
        method: "PUT",
        headers: {},
        body,
      }),
    ).toMatchObject({ kind: "fetch", body });
  });

  it("accepts a Blob body", () => {
    const body = new Blob([new Uint8Array([1, 2, 3])]);
    expect(
      parseArtifactRequest({
        tag: BRIDGE_TAG,
        kind: "fetch",
        id: "req-1",
        path: "/api/workspace/raw",
        method: "PUT",
        headers: {},
        body,
      }),
    ).toMatchObject({ kind: "fetch", body });
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
    expect(parseArtifactRequest(data)).toBeNull();
  });
});

interface Harness {
  bridge: WorkbenchBridge;
  frame: FrameTarget & { posted: Record<string, unknown>[] };
  deps: BridgeDeps;
  emit: (msg: ServerMessage) => void;
  sent: string[];
  escapes: () => number;
}

function harness(overrides: Partial<BridgeDeps> = {}): Harness {
  const posted: Record<string, unknown>[] = [];
  const frame = {
    posted,
    postMessage: (message: unknown) => posted.push(message as Record<string, unknown>),
  };
  let listener: ((msg: ServerMessage) => void) | null = null;
  const sent: string[] = [];
  let escapes = 0;
  const deps: BridgeDeps = {
    origin: ORIGIN,
    fetch: vi.fn(() => Promise.resolve(new Response('{"ok":true}', { status: 200 }))),
    hasUserActivation: () => true,
    isConnected: () => true,
    sendToAgent: (content) => sent.push(content),
    onEscape: () => {
      escapes += 1;
    },
    onFrame: (l) => {
      listener = l;
      return () => {
        listener = null;
      };
    },
    ...overrides,
  };
  const bridge = new WorkbenchBridge("chart", ARTIFACTS, () => frame, deps);
  bridge.start();
  return { bridge, frame, deps, emit: (msg) => listener?.(msg), sent, escapes: () => escapes };
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input.url;
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
  it("ignores messages from anything but the artifact's frame on the artifacts origin", async () => {
    const h = harness();
    await h.bridge.handleMessage({}, ARTIFACTS, fetchMsg("/api/status"));
    await h.bridge.handleMessage(h.frame, "https://evil.example", fetchMsg("/api/status"));
    expect(h.deps.fetch).not.toHaveBeenCalled();
    expect(h.frame.posted).toEqual([]);
  });

  it("relays an allowed request and its response", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, fetchMsg("/api/status"));
    expect(h.deps.fetch).toHaveBeenCalledWith(
      "/api/status",
      expect.objectContaining({ method: "GET", headers: { "X-Residuum-Artifact": "chart" } }),
    );
    const reply = h.frame.posted[0];
    expect(reply).toMatchObject({ tag: BRIDGE_TAG, kind: "result", id: "req-1" });
    const result = reply?.result as RelayedResponse;
    expect(result.status).toBe(200);
    expect(new TextDecoder().decode(result.body)).toBe('{"ok":true}');
  });

  it("relays a binary ArrayBuffer body unchanged, byte for byte", async () => {
    const bytes = new Uint8Array([0, 1, 2, 253, 254, 255]);
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/workspace/raw?path=image.bin", "PUT"),
      body: bytes.buffer,
    });
    expect(h.deps.fetch).toHaveBeenCalledWith(
      "/api/workspace/raw?path=image.bin",
      expect.objectContaining({ method: "PUT", body: bytes.buffer }),
    );
  });

  it("relays a Blob body unchanged", async () => {
    const blob = new Blob([new Uint8Array([9, 9, 9])]);
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/workspace/raw?path=image.bin", "PUT"),
      body: blob,
    });
    expect(h.deps.fetch).toHaveBeenCalledWith(
      "/api/workspace/raw?path=image.bin",
      expect.objectContaining({ method: "PUT", body: blob }),
    );
  });

  it("stamps the artifact identity header, overwriting any spoofed value regardless of casing", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/status"),
      headers: { "x-residuum-ARTIFACT": "someone-else", "X-Keep-Me": "yes" },
    });
    expect(h.deps.fetch).toHaveBeenCalledWith(
      "/api/status",
      expect.objectContaining({
        headers: { "X-Keep-Me": "yes", "X-Residuum-Artifact": "chart" },
      }),
    );
  });

  it("relays at most 8 requests at a time and serves the rest in FIFO order", async () => {
    const startOrder: number[] = [];
    const release: (() => void)[] = [];
    const fetchImpl = vi.fn((input: RequestInfo | URL) => {
      const url = requestUrl(input);
      const n = Number(new URL(url, "http://artifact.invalid").searchParams.get("n"));
      startOrder.push(n);
      return new Promise<Response>((resolve) => {
        release.push(() => {
          resolve(new Response("{}", { status: 200 }));
        });
      });
    });
    const h = harness({ fetch: fetchImpl });
    const total = 10;
    const handled = Promise.all(
      Array.from({ length: total }, (_, n) =>
        h.bridge.handleMessage(h.frame, ARTIFACTS, {
          ...fetchMsg(`/api/status?n=${n}`),
          id: `req-${n}`,
        }),
      ),
    );

    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(8);
    });
    expect(startOrder).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);

    release.shift()?.();
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(9);
    });
    expect(startOrder.at(-1)).toBe(8);

    release.shift()?.();
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(10);
    });
    expect(startOrder.at(-1)).toBe(9);

    while (release.length > 0) release.shift()?.();
    await handled;
  });

  it("retries a relay 'agent overloaded' 503 up to 3 times, then gives up", async () => {
    const sleeps: number[] = [];
    const fetchImpl = vi.fn(() =>
      Promise.resolve(new Response("agent overloaded", { status: 503 })),
    );
    const h = harness({
      fetch: fetchImpl,
      sleep: (ms) => {
        sleeps.push(ms);
        return Promise.resolve();
      },
    });
    await h.bridge.handleMessage(h.frame, ARTIFACTS, fetchMsg("/api/status"));
    expect(fetchImpl).toHaveBeenCalledTimes(4);
    expect(sleeps).toHaveLength(3);
    expect(sleeps[0]).toBeGreaterThanOrEqual(500);
    expect(sleeps[1]).toBeGreaterThan(sleeps[0] ?? 0);
    expect(sleeps[2]).toBeGreaterThan(sleeps[1] ?? 0);
    const result = h.frame.posted[0]?.result as RelayedResponse;
    expect(result.status).toBe(503);
  });

  it("does not retry a 503 whose body isn't the relay's overload text", async () => {
    const fetchImpl = vi.fn(() => Promise.resolve(new Response("internal error", { status: 503 })));
    const h = harness({ fetch: fetchImpl, sleep: () => Promise.resolve() });
    await h.bridge.handleMessage(h.frame, ARTIFACTS, fetchMsg("/api/status"));
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    const result = h.frame.posted[0]?.result as RelayedResponse;
    expect(result.status).toBe(503);
  });

  it("answers a blocked request with a 403 without calling the gateway", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, fetchMsg("/api/shutdown", "POST"));
    expect(h.deps.fetch).not.toHaveBeenCalled();
    const result = h.frame.posted[0]?.result as RelayedResponse;
    expect(result.status).toBe(403);
    expect(JSON.parse(new TextDecoder().decode(result.body))).toHaveProperty("error");
  });

  it("reports a network failure as an error", async () => {
    const h = harness({ fetch: vi.fn(() => Promise.reject(new TypeError("offline"))) });
    await h.bridge.handleMessage(h.frame, ARTIFACTS, fetchMsg("/api/status"));
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("sends to the agent after a user gesture, labelled with the artifact", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
      tag: BRIDGE_TAG,
      kind: "send",
      id: "s1",
      content: " pick B ",
    });
    expect(h.sent).toEqual(['[From workbench artifact "chart"]\npick B']);
    expect(h.frame.posted[0]).toMatchObject({ id: "s1", result: null });
  });

  it("refuses to message the agent without a user gesture", async () => {
    const h = harness({ hasUserActivation: () => false });
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
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
    await h.bridge.handleMessage(h.frame, ARTIFACTS, {
      tag: BRIDGE_TAG,
      kind: "send",
      id: "s1",
      content: "hi",
    });
    expect(h.sent).toEqual([]);
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("forwards server frames only after the artifact subscribes, until its document changes", async () => {
    const h = harness();
    const frame: ServerMessage = { type: "artifact_updated", name: "chart" };
    h.emit(frame);
    expect(h.frame.posted).toEqual([]);

    await h.bridge.handleMessage(h.frame, ARTIFACTS, { tag: BRIDGE_TAG, kind: "subscribe" });
    h.emit(frame);
    expect(h.frame.posted).toEqual([{ tag: BRIDGE_TAG, kind: "event", frame }]);

    h.emit({ type: "pong" });
    expect(h.frame.posted).toHaveLength(1);

    h.bridge.documentChanged();
    h.emit(frame);
    expect(h.frame.posted).toHaveLength(1);
  });

  it("passes an unhandled Esc from the artifact to the page", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, { tag: BRIDGE_TAG, kind: "escape" });
    expect(h.escapes()).toBe(1);
    await h.bridge.handleMessage({}, ARTIFACTS, { tag: BRIDGE_TAG, kind: "escape" });
    expect(h.escapes()).toBe(1);
  });
});
