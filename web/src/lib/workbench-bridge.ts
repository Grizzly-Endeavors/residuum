// ── Workbench bridge ─────────────────────────────────────────────────
//
// Workbench artifacts run on their own origin (the artifacts listener), so
// they can't call the gateway's API themselves: another origin can't read
// its responses, and the gateway rejects its writes. The SDK injected into
// each artifact page (assets/workbench/sdk.js) posts requests here instead,
// and this bridge makes them on the artifact's behalf. This is the one place
// that decides what an artifact may reach: most of the API is open, but
// routes that change secrets, credentials, raw config, or Residuum's own
// lifecycle are refused, and messages to the agent need a real click or key
// press in the artifact.

import type { ServerMessage } from "./types";

/** Tag on every message between the SDK and the bridge. Matches sdk.js. */
export const BRIDGE_TAG = "residuum-workbench";

/** Longest message an artifact may send to the agent. */
export const MAX_AGENT_MESSAGE_CHARS = 20_000;

/** Header the bridge stamps on every relayed request, identifying the artifact to the gateway. */
export const ARTIFACT_HEADER = "X-Residuum-Artifact";

/** How many ordinary requests one bridge relays at once; the rest queue in order. */
const MAX_CONCURRENT_REQUESTS = 8;

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

interface SendRequest {
  kind: "send";
  id: string;
  content: string;
}

interface SubscribeRequest {
  kind: "subscribe";
}

/** The user pressed Esc in the artifact and the artifact didn't handle it. */
interface EscapeRequest {
  kind: "escape";
}

type ArtifactRequest = FetchRequest | SendRequest | SubscribeRequest | EscapeRequest;

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
    case "send":
      if (typeof data.id === "string" && typeof data.content === "string") {
        return { kind: "send", id: data.id, content: data.content };
      }
      return null;
    case "subscribe":
      return { kind: "subscribe" };
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

export interface BridgeDeps {
  /** The gateway's origin (the web UI's own). */
  origin: string;
  fetch: typeof fetch;
  /** Whether the user clicked or typed recently; activation in a frame propagates to its parent. */
  hasUserActivation: () => boolean;
  /** Whether messages can reach the agent right now. */
  isConnected: () => boolean;
  /** Send a chat message to the main agent as the user. */
  sendToAgent: (content: string) => void;
  /** Observe server frames; returns a function that stops observing. */
  onFrame: (listener: (msg: ServerMessage) => void) => () => void;
  /** The user pressed Esc inside the artifact and the artifact left it unhandled. */
  onEscape: () => void;
  /** Waits before a retry. Defaults to a real timer; injectable for tests. */
  sleep?: (ms: number) => Promise<void>;
}

export class WorkbenchBridge {
  private subscribed = false;
  private stopObserving: (() => void) | null = null;
  private readonly requests = new ConcurrencyLimiter(MAX_CONCURRENT_REQUESTS);
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

  /** Start forwarding server frames to subscribed artifacts. */
  start(): void {
    this.stopObserving ??= this.deps.onFrame((frame) => {
      // Keepalive pongs are transport noise, not events an artifact can act on.
      if (this.subscribed && frame.type !== "pong") this.post({ kind: "event", frame });
    });
  }

  stop(): void {
    this.stopObserving?.();
    this.stopObserving = null;
    this.subscribed = false;
  }

  /**
   * The frame loaded a new document (a reload, or the artifact navigated its
   * frame). It must subscribe again before it receives frames.
   */
  documentChanged(): void {
    this.subscribed = false;
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
      case "subscribe":
        this.subscribed = true;
        return;
      case "escape":
        this.deps.onEscape();
        return;
      case "send":
        this.handleSend(request);
        return;
      case "fetch":
        await this.handleFetch(request);
        return;
    }
  }

  private handleSend(request: SendRequest): void {
    const content = request.content.trim();
    if (content === "") {
      this.reply(request.id, { error: "residuum.send needs a non-empty message." });
      return;
    }
    if (content.length > MAX_AGENT_MESSAGE_CHARS) {
      this.reply(request.id, {
        error: `Messages to the agent are limited to ${MAX_AGENT_MESSAGE_CHARS} characters.`,
      });
      return;
    }
    if (!this.deps.hasUserActivation()) {
      this.reply(request.id, {
        error:
          "Artifacts can only message the agent right after a click or key press in the artifact. Call residuum.send from an event handler.",
      });
      return;
    }
    if (!this.deps.isConnected()) {
      this.reply(request.id, {
        error: "Residuum isn't connected right now, so the message wasn't sent. Try again shortly.",
      });
      return;
    }
    this.deps.sendToAgent(`[From workbench artifact "${this.artifact}"]\n${content}`);
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

    let resp: Response;
    try {
      resp = await this.requests.run(() => this.relayWithRetry(check.url, request));
    } catch (err) {
      // eslint-disable-next-line no-console -- the tool gets a plain-language error; the raw cause is for developers
      console.error("workbench bridge request failed", request.method, check.url, err);
      this.reply(request.id, {
        error: "Couldn't reach Residuum. Check that it's running, then try again.",
      });
      return;
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
  private async relayWithRetry(url: string, request: FetchRequest): Promise<Response> {
    for (let attempt = 0; ; attempt += 1) {
      const resp = await this.deps.fetch(url, {
        method: request.method,
        headers: this.withArtifactHeader(request.headers),
        body: request.body,
        credentials: "same-origin",
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
