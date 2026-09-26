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
    ["POST", "/api/model/complete"],
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
  escapes: () => number;
  /** Every watch set handed to the coordinator, in order. */
  watchSets: (readonly string[])[];
  setConnected: (connected: boolean) => void;
}

function harness(overrides: Partial<BridgeDeps> = {}): Harness {
  const posted: Record<string, unknown>[] = [];
  const frame = {
    posted,
    postMessage: (message: unknown) => posted.push(message as Record<string, unknown>),
  };
  let listener: ((msg: ServerMessage) => void) | null = null;
  let connectionListener: ((connected: boolean) => void) | null = null;
  const watchSets: (readonly string[])[] = [];
  let escapes = 0;
  const deps: BridgeDeps = {
    onConnectionChange: (l) => {
      connectionListener = l;
      return () => {
        connectionListener = null;
      };
    },
    watchWorkspace: (prefixes) => watchSets.push(prefixes),
    origin: ORIGIN,
    fetch: vi.fn(() => Promise.resolve(new Response('{"ok":true}', { status: 200 }))),
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
  return {
    bridge,
    frame,
    deps,
    emit: (msg) => listener?.(msg),
    escapes: () => escapes,
    watchSets,
    setConnected: (connected) => connectionListener?.(connected),
  };
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

  it("relays model calls in their own lane, separate from the 8-request limit", async () => {
    const release: (() => void)[] = [];
    const fetchImpl = vi.fn(
      () =>
        new Promise<Response>((resolve) =>
          release.push(() => {
            resolve(new Response("{}", { status: 200 }));
          }),
        ),
    );
    const h = harness({ fetch: fetchImpl });

    // Fill the ordinary-request lane with 8 requests that never resolve.
    const ordinary = Promise.all(
      Array.from({ length: 8 }, (_, n) =>
        h.bridge.handleMessage(h.frame, ARTIFACTS, { ...fetchMsg("/api/status"), id: `ord-${n}` }),
      ),
    );
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(8);
    });

    // A model call still goes out immediately: it has its own lane.
    const modelCall = h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/model/complete", "POST"),
      id: "model-1",
    });
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(9);
    });

    while (release.length > 0) release.shift()?.();
    await Promise.all([ordinary, modelCall]);
  });

  it("caps model calls at 4 concurrent, queueing the rest", async () => {
    const release: (() => void)[] = [];
    const fetchImpl = vi.fn(
      () =>
        new Promise<Response>((resolve) =>
          release.push(() => {
            resolve(new Response("{}", { status: 200 }));
          }),
        ),
    );
    const h = harness({ fetch: fetchImpl });

    const calls = Promise.all(
      Array.from({ length: 5 }, (_, n) =>
        h.bridge.handleMessage(h.frame, ARTIFACTS, {
          ...fetchMsg("/api/model/complete", "POST"),
          id: `model-${n}`,
        }),
      ),
    );
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(4);
    });
    // Only 4 are actually fetching; the 5th is queued for a lane slot but is
    // still tracked (so it can be cancelled before it ever calls fetch).
    expect(h.bridge.modelCallsInFlight).toBe(5);

    // Releasing one frees a lane slot for the 5th, queued call.
    release.shift()?.();
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(5);
    });

    while (release.length > 0) release.shift()?.();
    await calls;
  });

  it("cancelModelCalls aborts in-flight model calls and rejects the SDK promise", async () => {
    const fetchImpl = vi.fn(
      (_input: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => {
            reject(Object.assign(new Error("aborted"), { name: "AbortError" }));
          });
        }),
    );
    const h = harness({ fetch: fetchImpl });

    const pending = h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/model/complete", "POST"),
      id: "model-1",
    });
    await vi.waitFor(() => {
      expect(h.bridge.modelCallsInFlight).toBe(1);
    });

    h.bridge.cancelModelCalls();
    await pending;

    expect(h.bridge.modelCallsInFlight).toBe(0);
    expect(h.frame.posted[0]).toMatchObject({ id: "model-1" });
    expect(h.frame.posted[0]).toHaveProperty("error");
  });

  it("reports the in-flight model call count through onModelCallsChanged as calls start and finish", async () => {
    const release: (() => void)[] = [];
    const fetchImpl = vi.fn(
      () =>
        new Promise<Response>((resolve) =>
          release.push(() => {
            resolve(new Response("{}", { status: 200 }));
          }),
        ),
    );
    const counts: number[] = [];
    const h = harness({ fetch: fetchImpl, onModelCallsChanged: (count) => counts.push(count) });

    const calls = Promise.all(
      Array.from({ length: 2 }, (_, n) =>
        h.bridge.handleMessage(h.frame, ARTIFACTS, {
          ...fetchMsg("/api/model/complete", "POST"),
          id: `model-${n}`,
        }),
      ),
    );
    await vi.waitFor(() => {
      expect(fetchImpl).toHaveBeenCalledTimes(2);
    });
    expect(counts).toEqual([1, 2]);

    while (release.length > 0) release.shift()?.();
    await calls;
    expect(counts).toEqual([1, 2, 1, 0]);
  });

  it("cancelModelCalls leaves ordinary requests untouched", async () => {
    const ordinaryReleased: (() => void)[] = [];
    const fetchImpl = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      if (requestUrl(input).includes("/api/model/complete")) {
        return new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => {
            reject(Object.assign(new Error("aborted"), { name: "AbortError" }));
          });
        });
      }
      return new Promise<Response>((resolve) => {
        ordinaryReleased.push(() => {
          resolve(new Response("{}", { status: 200 }));
        });
      });
    });
    const h = harness({ fetch: fetchImpl });

    const ordinary = h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/status"),
      id: "ord-1",
    });
    const modelCall = h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/model/complete", "POST"),
      id: "model-1",
    });
    await vi.waitFor(() => {
      expect(h.bridge.modelCallsInFlight).toBe(1);
    });

    h.bridge.cancelModelCalls();
    await modelCall;
    expect(h.frame.posted.find((m) => m.id === "model-1")).toHaveProperty("error");
    expect(h.frame.posted.find((m) => m.id === "ord-1")).toBeUndefined();

    ordinaryReleased.shift()?.();
    await ordinary;
    expect(h.frame.posted.find((m) => m.id === "ord-1")?.result).toMatchObject({ status: 200 });
  });

  it("aborts in-flight model calls when the bridge is torn down", async () => {
    const fetchImpl = vi.fn(
      (_input: RequestInfo | URL, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => {
            reject(Object.assign(new Error("aborted"), { name: "AbortError" }));
          });
        }),
    );
    const h = harness({ fetch: fetchImpl });

    const pending = h.bridge.handleMessage(h.frame, ARTIFACTS, {
      ...fetchMsg("/api/model/complete", "POST"),
      id: "model-1",
    });
    await vi.waitFor(() => {
      expect(h.bridge.modelCallsInFlight).toBe(1);
    });

    h.bridge.stop();
    await pending;

    expect(h.bridge.modelCallsInFlight).toBe(0);
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

describe("WorkbenchBridge change feed", () => {
  const watchMsg = (prefixes: unknown[], id = "req-w"): Record<string, unknown> => ({
    tag: BRIDGE_TAG,
    kind: "watch",
    id,
    prefixes,
  });
  const ready = { tag: BRIDGE_TAG, kind: "ready" };
  const changed = (...paths: string[]): ServerMessage => ({
    type: "workspace_changed",
    changes: paths.map((path) => ({ path, kind: "modified" as const })),
  });
  const events = (h: Harness): unknown[] =>
    h.frame.posted.filter((m) => m.kind === "event").map((m) => m.frame);

  it("hands the artifact's normalized watch set to the coordinator", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["wiki/", "./inbox/user", "wiki"]));
    expect(h.watchSets).toEqual([["inbox/user", "wiki"]]);
    expect(h.frame.posted).toContainEqual({
      tag: BRIDGE_TAG,
      kind: "result",
      id: "req-w",
      result: null,
    });

    h.bridge.stop();
    expect(h.watchSets.at(-1)).toEqual([]);
  });

  it("refuses a prefix outside the workspace and keeps the current set", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["wiki"]));
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["notes", "../secrets"], "req-bad"));
    expect(h.watchSets).toEqual([["wiki"]]);
    expect(h.frame.posted.find((m) => m.id === "req-bad")).toHaveProperty("error");
  });

  it("delivers only the changes under the watched prefixes, by whole segments", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["wiki"]));
    h.emit(changed("wikipedia/a.md", "notes/b.md"));
    expect(events(h)).toEqual([]);

    h.emit(changed("wiki/a.md", "wikipedia/b.md", "wiki/deep/c.md"));
    expect(events(h)).toEqual([changed("wiki/a.md", "wiki/deep/c.md")]);
  });

  it("delivers nothing to an artifact that watches nothing, even when subscribed", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, { tag: BRIDGE_TAG, kind: "subscribe" });
    h.emit(changed("wiki/a.md"));
    h.emit({ type: "workspace_resync", reason: "overflow" });
    h.emit({ type: "workspace_watch_unavailable", message: "off" });
    expect(events(h)).toEqual([]);
  });

  it("passes server resyncs to a watching artifact", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg([""]));
    h.emit({ type: "workspace_resync", reason: "watcher_restarted" });
    expect(events(h)).toEqual([{ type: "workspace_resync", reason: "watcher_restarted" }]);
  });

  it("sends connection frames and a reconnect resync after the socket returns", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["wiki"]));
    h.setConnected(false);
    h.setConnected(true);
    expect(events(h)).toEqual([
      { type: "connection", state: "disconnected" },
      { type: "connection", state: "connected" },
      { type: "workspace_resync", reason: "reconnected" },
    ]);
  });

  it("sends connection frames without a resync to an artifact that only subscribed", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, { tag: BRIDGE_TAG, kind: "subscribe" });
    h.setConnected(false);
    h.setConnected(true);
    expect(events(h)).toEqual([
      { type: "connection", state: "disconnected" },
      { type: "connection", state: "connected" },
    ]);
  });

  it("keeps what a new document set up while it loaded, and forgets the previous document's", async () => {
    const h = harness();
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["old"]));

    // The reloaded page announces itself and watches before its load event.
    await h.bridge.handleMessage(h.frame, ARTIFACTS, ready);
    await h.bridge.handleMessage(h.frame, ARTIFACTS, { tag: BRIDGE_TAG, kind: "subscribe" });
    await h.bridge.handleMessage(h.frame, ARTIFACTS, watchMsg(["wiki"]));
    h.bridge.documentChanged();
    expect(h.watchSets).toEqual([["old"], [], ["wiki"]]);

    h.emit(changed("wiki/a.md", "old/b.md"));
    h.emit({ type: "artifact_updated", name: "chart" });
    expect(events(h)).toEqual([changed("wiki/a.md"), { type: "artifact_updated", name: "chart" }]);

    // A page without the SDK never announces itself: its load clears everything.
    h.bridge.documentChanged();
    expect(h.watchSets.at(-1)).toEqual([]);
  });
});
