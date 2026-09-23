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
    path: /^\/api\/(config|providers|mcp)\/raw(\/|$)/,
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
  {
    path: /^\/api\/workbench\/artifacts\//,
    methods: "writes",
    reason: "Workbench artifacts can't delete workbench artifacts.",
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

interface FetchRequest {
  kind: "fetch";
  id: string;
  path: string;
  method: string;
  headers: Record<string, string>;
  body: string | null;
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
        (data.body === null || typeof data.body === "string")
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
}

export class WorkbenchBridge {
  private subscribed = false;
  private stopObserving: (() => void) | null = null;

  constructor(
    private readonly artifact: string,
    /** The artifacts origin; the only origin the bridge listens to or posts to. */
    private readonly frameOrigin: string,
    private readonly target: () => FrameTarget | null,
    private readonly deps: BridgeDeps,
  ) {}

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
      resp = await this.deps.fetch(check.url, {
        method: request.method,
        headers: request.headers,
        body: request.body,
        credentials: "same-origin",
      });
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
