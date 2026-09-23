// ── Workbench bridge ─────────────────────────────────────────────────
//
// Workbench artifacts run on their own origin (the artifacts listener), so
// they can't call the gateway's API themselves: another origin can't read
// its responses, and the gateway rejects its writes. The SDK injected into
// each artifact page (assets/workbench/sdk.js) posts requests here instead,
// and this bridge makes them on the artifact's behalf. This is the one place
// that decides what an artifact may reach: most of the API is open, but
// routes that change secrets, credentials, raw config, or Residuum's own
// lifecycle are refused.
//
// The bridge also carries the workspace change feed: it hands the artifact's
// watched prefixes to the WebSocket coordinator, delivers only the changes
// under them, and tells the artifact when the connection drops and returns.

import type { ServerMessage, WorkspaceChange } from "./types";
import { changesUnder, normalizeWatchPrefix } from "./workspace-watch";

/** Tag on every message between the SDK and the bridge. Matches sdk.js. */
export const BRIDGE_TAG = "residuum-workbench";

/** Header the bridge stamps on every relayed request, identifying the artifact to the gateway. */
export const ARTIFACT_HEADER = "X-Residuum-Artifact";

/** How many ordinary requests one bridge relays at once; the rest queue in order. */
const MAX_CONCURRENT_REQUESTS = 8;

/**
 * How many model calls (`POST /api/model/complete`) one bridge relays at
 * once, kept separate from `MAX_CONCURRENT_REQUESTS` so a burst of slow
 * model calls never holds up an artifact's ordinary requests.
 */
const MAX_CONCURRENT_MODEL_CALLS = 4;

/** The route model calls are identified by, for their own concurrency lane and abort tracking. */
const MODEL_COMPLETE_PATH = "/api/model/complete";

/** How many times a relay `agent overloaded` 503 is retried before giving up. */
const MAX_OVERLOADED_RETRIES = 3;

/** First retry's base delay; later retries double it before jitter. */
const RETRY_BASE_MS = 500;

/** The relay's exact body for a request it refused rather than forwarded. */
const OVERLOADED_BODY = "agent overloaded";

const ALLOWED_METHODS = new Set(["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"]);
const READ_METHODS = new Set(["GET", "HEAD"]);

interface BlockRule {
  path: RegExp;
  /** `writes` blocks everything except GET/HEAD. */
  methods: "all" | "writes";
  reason: string;
}

const BLOCKED_ROUTES: BlockRule[] = [
  {
    path: /^\/api\/secrets(\/|$)/,
    methods: "writes",
    reason: "Workbench artifacts can't change secrets. Manage them in Settings.",
  },
  {
    path: /^\/api\/agent-keys(\/|$)/,
    methods: "writes",
    reason: "Workbench artifacts can't change agent keys. Manage them in Settings.",
  },
  {
    path: /^\/api\/(config|providers)\/raw(\/|$)/,
    methods: "all",
    reason:
      "Workbench artifacts can't read or change raw configuration files, since they can hold credentials.",
  },
  {
    path: /^\/api\/config\/complete-setup(\/|$)/,
    methods: "all",
    reason: "Workbench artifacts can't run setup.",
  },
  {
    path: /^\/api\/(shutdown|update\/(check|apply|restart))(\/|$)/,
    methods: "all",
    reason: "Workbench artifacts can't shut down, update, or restart Residuum.",
  },
  {
    path: /^\/api\/cloud\/disconnect(\/|$)/,
    methods: "all",
    reason: "Workbench artifacts can't disconnect remote access.",
  },
  {
    path: /^\/api\/tracing\//,
    methods: "writes",
    reason: "Workbench artifacts can't change tracing or send diagnostics.",
  },
];

export type RequestCheck =
  | { allowed: true; url: string }
  | { allowed: false; status: number; reason: string };

/**
 * Decide whether an artifact may make this request. `path` must be a path on
 * the gateway under `/api/`; anything that resolves elsewhere is refused.
 */
export function checkArtifactRequest(method: string, path: string, origin: string): RequestCheck {
  if (!ALLOWED_METHODS.has(method)) {
    return {
      allowed: false,
      status: 405,
      reason: `Workbench artifacts can't use ${method} requests.`,
    };
  }
  let url: URL;
  try {
    url = new URL(path, origin);
  } catch {
    return { allowed: false, status: 400, reason: `"${path}" isn't a valid path.` };
  }
  if (url.origin !== origin || !url.pathname.startsWith("/api/")) {
    return {
      allowed: false,
      status: 400,
      reason: "residuum.fetch only reaches Residuum's API: use a path starting with /api/.",
    };
  }

  let decoded: string;
  try {
    decoded = decodeURIComponent(url.pathname);
  } catch {
    return { allowed: false, status: 400, reason: `"${path}" isn't a valid path.` };
  }
  const isRead = READ_METHODS.has(method);
  for (const rule of BLOCKED_ROUTES) {
    if (rule.methods === "writes" && isRead) continue;
    if (rule.path.test(url.pathname) || rule.path.test(decoded)) {
      return { allowed: false, status: 403, reason: rule.reason };
    }
  }
  return { allowed: true, url: url.pathname + url.search };
}

// ── Messages ─────────────────────────────────────────────────────────

/**
 * A relayed request's body. The SDK sends binary bodies (`ArrayBuffer`,
 * typed arrays, `Blob`) unchanged rather than JSON-encoding them; the bridge
 * relays whichever shape it receives without inspecting it further.
 */
type FetchRequestBody = string | ArrayBuffer | Blob | null;

interface FetchRequest {
  kind: "fetch";
  id: string;
  path: string;
  method: string;
  headers: Record<string, string>;
  body: FetchRequestBody;
}

interface SubscribeRequest {
  kind: "subscribe";
}

/** Replace the artifact's watched workspace prefixes. */
interface WatchRequest {
  kind: "watch";
  id: string;
  prefixes: string[];
}

/** The SDK started in a new document; anything the previous one set up is gone. */
interface ReadyRequest {
  kind: "ready";
}

/** The user pressed Esc in the artifact and the artifact didn't handle it. */
interface EscapeRequest {
  kind: "escape";
}

type ArtifactRequest =
  | FetchRequest
  | SubscribeRequest
  | WatchRequest
  | ReadyRequest
  | EscapeRequest;

/** Most prefixes one artifact may watch; the gateway refuses more. */
const MAX_WATCH_PREFIXES = 256;

/** A relayed response, rebuilt into a `Response` by the SDK. */
export interface RelayedResponse {
  status: number;
  statusText: string;
  headers: [string, string][];
  body: ArrayBuffer;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringMap(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((v) => typeof v === "string");
}

/** Whether `value` is one of the body shapes `residuum.fetch` may send unencoded. */
function isValidFetchBody(value: unknown): value is FetchRequestBody {
  return (
    value === null ||
    typeof value === "string" ||
    value instanceof ArrayBuffer ||
    (typeof Blob !== "undefined" && value instanceof Blob)
  );
}

/** Validate an SDK message. `null` when it isn't a well-formed bridge request. */
export function parseArtifactRequest(data: unknown): ArtifactRequest | null {
  if (!isRecord(data) || data.tag !== BRIDGE_TAG) return null;
  switch (data.kind) {
    case "fetch":
      if (
        typeof data.id === "string" &&
        typeof data.path === "string" &&
        typeof data.method === "string" &&
        isStringMap(data.headers) &&
        isValidFetchBody(data.body)
      ) {
        return {
          kind: "fetch",
          id: data.id,
          path: data.path,
          method: data.method,
          headers: data.headers,
          body: data.body,
        };
      }
      return null;
    case "subscribe":
      return { kind: "subscribe" };
    case "watch":
      if (
        typeof data.id === "string" &&
        Array.isArray(data.prefixes) &&
        data.prefixes.every((p) => typeof p === "string")
      ) {
        return { kind: "watch", id: data.id, prefixes: data.prefixes };
      }
      return null;
    case "ready":
      return { kind: "ready" };
    case "escape":
      return { kind: "escape" };
    default:
      return null;
  }
}

// ── Bridge ───────────────────────────────────────────────────────────

/** The frame's window, as far as the bridge needs it. */
export interface FrameTarget {
  postMessage(message: unknown, targetOrigin: string, transfer?: Transferable[]): void;
}

/**
 * Caps how many relayed requests run at once, queueing the rest in the order
 * they arrived. The relay refuses an instance's requests past 50 in flight
 * (`503 agent overloaded`); staying well under that per bridge means one
 * artifact's bulk load can't crowd out its other calls.
 */
class ConcurrencyLimiter {
  private active = 0;
  private readonly queue: (() => void)[] = [];

  constructor(private readonly limit: number) {}

  async run<T>(task: () => Promise<T>): Promise<T> {
    if (this.active >= this.limit) {
      // Wait for a finishing task to hand over its slot.
      await new Promise<void>((resolve) => this.queue.push(resolve));
    } else {
      this.active += 1;
    }
    try {
      return await task();
    } finally {
      // FIFO: the slot passes straight to whoever queued first, so a caller
      // arriving in between can't take it and push the count past the limit.
      const next = this.queue.shift();
      if (next) next();
      else this.active -= 1;
    }
  }
}

/** A real timer, used unless a test injects `BridgeDeps.sleep`. */
const defaultSleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Exponential backoff with jitter for the attempt-th retry (1-based),
 * starting near `RETRY_BASE_MS`.
 */
function retryDelayMs(attempt: number): number {
  const base = RETRY_BASE_MS * 2 ** (attempt - 1);
  return base + Math.random() * base * 0.2;
}

/**
 * Whether `resp` is the relay's own overload refusal (a `503` with exactly its
 * `agent overloaded` body), not some other `503` the gateway itself returned.
 * Reads a clone so the original body is still available to the caller.
 */
async function isRelayOverloaded(resp: Response): Promise<boolean> {
  if (resp.status !== 503) return false;
  try {
    return (await resp.clone().text()).trim() === OVERLOADED_BODY;
  } catch {
    return false;
  }
}

/** Whether a relayed request's resolved URL is a model call, by route (design §10). */
function isModelCompletePath(url: string): boolean {
  const path = url.split("?", 1)[0];
  return path === MODEL_COMPLETE_PATH;
}

/** Whether `err` is a `fetch` abort, from this bridge cancelling the request's signal. */
function isAbortError(err: unknown): boolean {
  return (
    typeof err === "object" && err !== null && (err as { name?: unknown }).name === "AbortError"
  );
}

export interface BridgeDeps {
  /** The gateway's origin (the web UI's own). */
  origin: string;
  fetch: typeof fetch;
  /** Observe server frames; returns a function that stops observing. */
  onFrame: (listener: (msg: ServerMessage) => void) => () => void;
  /** Observe the socket connecting and disconnecting; returns a function that stops observing. */
  onConnectionChange: (listener: (connected: boolean) => void) => () => void;
  /** Set the workspace prefixes the connection watches for this artifact. `[]` stops watching. */
  watchWorkspace: (prefixes: readonly string[]) => void;
  /** The user pressed Esc inside the artifact and the artifact left it unhandled. */
  onEscape: () => void;
  /** Waits before a retry. Defaults to a real timer; injectable for tests. */
  sleep?: (ms: number) => Promise<void>;
}

export class WorkbenchBridge {
  private subscribed = false;
  /** The artifact's watched workspace prefixes, normalized. */
  private watched: string[] = [];
  /** Whether the current document's SDK announced itself since the last frame load. */
  private readySinceLoad = false;
  /** Whether the socket dropped since the artifact last had a live connection. */
  private missedChanges = false;
  private stopObserving: (() => void) | null = null;
  private stopObservingConnection: (() => void) | null = null;
  private readonly requests = new ConcurrencyLimiter(MAX_CONCURRENT_REQUESTS);
  private readonly modelCalls = new ConcurrencyLimiter(MAX_CONCURRENT_MODEL_CALLS);
  /** Abort controllers for this frame's in-flight model calls, keyed by request id. */
  private readonly modelCallControllers = new Map<string, AbortController>();
  private readonly sleep: (ms: number) => Promise<void>;

  constructor(
    private readonly artifact: string,
    /** The artifacts origin; the only origin the bridge listens to or posts to. */
    private readonly frameOrigin: string,
    private readonly target: () => FrameTarget | null,
    private readonly deps: BridgeDeps,
  ) {
    this.sleep = deps.sleep ?? defaultSleep;
  }

  /** Start forwarding server frames to subscribed and watching artifacts. */
  start(): void {
    this.stopObserving ??= this.deps.onFrame((frame) => {
      this.forwardFrame(frame);
    });
    this.stopObservingConnection ??= this.deps.onConnectionChange((connected) => {
      this.connectionChanged(connected);
    });
  }

  /** Tears the bridge down: stops observing frames and aborts any model calls still in flight. */
  stop(): void {
    this.stopObserving?.();
    this.stopObserving = null;
    this.stopObservingConnection?.();
    this.stopObservingConnection = null;
    this.resetDocument();
    this.cancelModelCalls();
  }

  /** How many model calls this frame has in flight right now. */
  get modelCallsInFlight(): number {
    return this.modelCallControllers.size;
  }

  /** Aborts every model call currently in flight for this frame. */
  cancelModelCalls(): void {
    for (const controller of this.modelCallControllers.values()) controller.abort();
  }

  /**
   * The frame finished loading a document (a reload, or the artifact
   * navigated its frame). A document with the SDK announced itself while it
   * loaded, which already reset the bridge before it subscribed; any other
   * document starts with nothing subscribed or watched.
   */
  documentChanged(): void {
    if (!this.readySinceLoad) this.resetDocument();
    this.readySinceLoad = false;
  }

  /** Forget what the previous document subscribed to and watched. */
  private resetDocument(): void {
    this.subscribed = false;
    this.setWatched([]);
  }

  private setWatched(prefixes: string[]): void {
    if (prefixes.length === 0 && this.watched.length === 0) return;
    this.watched = prefixes;
    this.deps.watchWorkspace(prefixes);
  }

  private forwardFrame(frame: ServerMessage): void {
    if (frame.type === "workspace_changed") {
      this.deliverChanges(frame.changes);
    } else if (frame.type === "workspace_resync") {
      if (this.watched.length > 0) this.post({ kind: "event", frame });
    } else if (frame.type !== "workspace_watch_unavailable" && frame.type !== "pong") {
      // The web UI shows its own notice when live updates are off, and
      // keepalive pongs are transport noise, not events an artifact can act on.
      if (this.subscribed) this.post({ kind: "event", frame });
    }
  }

  /** Deliver the changes under the artifact's watched prefixes, if any. */
  private deliverChanges(changes: WorkspaceChange[]): void {
    if (this.watched.length === 0) return;
    const matching = changesUnder(changes, this.watched);
    if (matching.length === 0) return;
    this.post({ kind: "event", frame: { type: "workspace_changed", changes: matching } });
  }

  /**
   * Tell a listening artifact the socket's state. Changes made while it was
   * down are lost, so a watching artifact is told to resync once it's back.
   */
  private connectionChanged(connected: boolean): void {
    if (!connected) this.missedChanges = true;
    if (!this.subscribed && this.watched.length === 0) return;
    const state = connected ? "connected" : "disconnected";
    this.post({ kind: "event", frame: { type: "connection", state } });
    if (connected && this.missedChanges && this.watched.length > 0) {
      this.post({ kind: "event", frame: { type: "workspace_resync", reason: "reconnected" } });
    }
    if (connected) this.missedChanges = false;
  }

  /**
   * Handle a `message` event. Ignores anything not from the artifact's
   * frame, or from a page the frame navigated to on another origin.
   */
  async handleMessage(source: unknown, origin: string, data: unknown): Promise<void> {
    const frame = this.target();
    if (frame === null || source !== frame || origin !== this.frameOrigin) return;
    const request = parseArtifactRequest(data);
    if (request === null) return;

    switch (request.kind) {
      case "ready":
        this.resetDocument();
        this.readySinceLoad = true;
        return;
      case "subscribe":
        this.subscribed = true;
        return;
      case "watch":
        this.handleWatch(request);
        return;
      case "escape":
        this.deps.onEscape();
        return;
      case "fetch":
        await this.handleFetch(request);
        return;
    }
  }

  private handleWatch(request: WatchRequest): void {
    if (request.prefixes.length > MAX_WATCH_PREFIXES) {
      this.reply(request.id, {
        error: `An artifact can watch at most ${MAX_WATCH_PREFIXES} paths at once.`,
      });
      return;
    }
    const prefixes = new Set<string>();
    for (const prefix of request.prefixes) {
      const normalized = normalizeWatchPrefix(prefix);
      if (normalized === null) {
        this.reply(request.id, {
          error: `Can't watch "${prefix}": watch paths are relative to the workspace, like "wiki", and can't contain "..".`,
        });
        return;
      }
      prefixes.add(normalized);
    }
    this.setWatched([...prefixes].sort());
    this.reply(request.id, { result: null });
  }

  private async handleFetch(request: FetchRequest): Promise<void> {
    const check = checkArtifactRequest(request.method, request.path, this.deps.origin);
    if (!check.allowed) {
      const body = new TextEncoder().encode(JSON.stringify({ error: check.reason }));
      this.reply(request.id, {
        result: {
          status: check.status,
          statusText: "Blocked by the workbench",
          headers: [["content-type", "application/json"]],
          body: body.buffer,
        } satisfies RelayedResponse,
      });
      return;
    }

    // Model calls get their own concurrency lane (separate from ordinary
    // requests) and an abort signal, tracked per frame so the activity panel
    // and Stop page (design §9) can cancel them later.
    const isModelCall = isModelCompletePath(check.url);
    const limiter = isModelCall ? this.modelCalls : this.requests;
    const controller = isModelCall ? new AbortController() : null;
    if (controller) this.modelCallControllers.set(request.id, controller);

    let resp: Response;
    try {
      resp = await limiter.run(() => this.relayWithRetry(check.url, request, controller?.signal));
    } catch (err) {
      if (isAbortError(err)) {
        this.reply(request.id, { error: "The model call was cancelled." });
        return;
      }
      // eslint-disable-next-line no-console -- the tool gets a plain-language error; the raw cause is for developers
      console.error("workbench bridge request failed", request.method, check.url, err);
      this.reply(request.id, {
        error: "Couldn't reach Residuum. Check that it's running, then try again.",
      });
      return;
    } finally {
      if (controller) this.modelCallControllers.delete(request.id);
    }
    const body = await resp.arrayBuffer();
    const relayed: RelayedResponse = {
      status: resp.status,
      statusText: resp.statusText,
      headers: [...resp.headers.entries()],
      body,
    };
    this.reply(request.id, { result: relayed }, [body]);
  }

  /**
   * Makes the request, retrying a relay `agent overloaded` 503 up to
   * `MAX_OVERLOADED_RETRIES` times with backoff. The relay refuses these
   * before forwarding them, so retrying never duplicates a write. Any other
   * response, including other 503s, is returned as-is.
   */
  private async relayWithRetry(
    url: string,
    request: FetchRequest,
    signal?: AbortSignal,
  ): Promise<Response> {
    for (let attempt = 0; ; attempt += 1) {
      const resp = await this.deps.fetch(url, {
        method: request.method,
        headers: this.withArtifactHeader(request.headers),
        body: request.body,
        credentials: "same-origin",
        signal,
      });
      if (attempt >= MAX_OVERLOADED_RETRIES || !(await isRelayOverloaded(resp))) return resp;
      await this.sleep(retryDelayMs(attempt + 1));
    }
  }

  /**
   * Every relayed request carries the bridge's own artifact identity, never
   * one the artifact supplied, regardless of the header's casing.
   */
  private withArtifactHeader(headers: Record<string, string>): Record<string, string> {
    const stamped: Record<string, string> = {};
    for (const [key, value] of Object.entries(headers)) {
      if (key.toLowerCase() !== ARTIFACT_HEADER.toLowerCase()) stamped[key] = value;
    }
    stamped[ARTIFACT_HEADER] = this.artifact;
    return stamped;
  }

  private reply(
    id: string,
    outcome: { result: unknown } | { error: string },
    transfer: Transferable[] = [],
  ): void {
    this.post({ kind: "result", id, ...outcome }, transfer);
  }

  private post(message: Record<string, unknown>, transfer: Transferable[] = []): void {
    // If the frame navigated to another origin, the browser drops this.
    this.target()?.postMessage({ tag: BRIDGE_TAG, ...message }, this.frameOrigin, transfer);
  }
}
