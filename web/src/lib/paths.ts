// ── API base paths ───────────────────────────────────────────────────
//
// Every HTTP and WebSocket URL the app builds goes through here. The
// backend serves three scopes (see docs/systems-usage/hub-http.md):
//
//   /api/agents/{name}/...   everything belonging to one agent
//   /api/hub/...             one-per-process things: lifecycle, hub config, secrets
//   /api/team/...            the shared team layer: team files, workbench
//
// An agent-scoped path names its agent: every caller says which one. Nothing
// here remembers an agent between calls.

const AGENT_STORAGE_KEY = "residuum-last-agent";

/** The agent last opened, kept across reloads. */
export function readLastAgent(): string | null {
  try {
    const value = localStorage.getItem(AGENT_STORAGE_KEY);
    return value !== null && value !== "" ? value : null;
  } catch {
    return null;
  }
}

/** Remember `name` as the agent to open at `/`. */
export function rememberLastAgent(name: string): void {
  try {
    localStorage.setItem(AGENT_STORAGE_KEY, name);
  } catch {
    // localStorage unavailable
  }
}

/** Thrown when a call needs an agent and none was given. */
export class NoAgentSelectedError extends Error {
  constructor() {
    super("no agent is selected");
    this.name = "NoAgentSelectedError";
  }
}

/**
 * `agent` itself, or `NoAgentSelectedError` when there is none. For a caller
 * that holds a nullable agent (the bound agent before one is chosen) and
 * reaches an agent-scoped call that needs one.
 */
export function requireAgent(agent: string | null): string {
  if (agent === null) throw new NoAgentSelectedError();
  return agent;
}

/** `/api/agents/{name}` for `agent`. */
export function agentBase(agent: string): string {
  return `/api/agents/${encodeURIComponent(agent)}`;
}

/** An agent-scoped API path: `agentPath("atlas", "/status")` is `/api/agents/atlas/status`. */
export function agentPath(agent: string, sub: string): string {
  return `${agentBase(agent)}${sub}`;
}

/** A hub API path: `hubPath("/secrets")` is `/api/hub/secrets`. */
export function hubPath(sub: string): string {
  return `/api/hub${sub}`;
}

/** A team API path: `teamPath("/workbench/info")` is `/api/team/workbench/info`. */
export function teamPath(sub: string): string {
  return `/api/team${sub}`;
}

function wsUrl(path: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}${path}`;
}

/** WebSocket URL for one agent's connection. */
export function agentWsUrl(agent: string): string {
  return wsUrl(`${agentBase(agent)}/ws`);
}

/** WebSocket URL for the hub connection. */
export function hubWsUrl(): string {
  return wsUrl(hubPath("/ws"));
}

// ── Artifact-facing paths ────────────────────────────────────────────
//
// Workbench artifacts address the API by its unscoped paths (`/api/sessions`,
// `/api/secrets`), which stay stable for artifact authors. The bridge maps
// each to the scope it lives in.

const HUB_PREFIXES = [
  "/api/secrets",
  "/api/agent-keys",
  "/api/a2a/keys",
  "/api/cloud/",
  "/api/update/",
  "/api/tracing/",
  "/api/shutdown",
  "/api/system/timezone",
  "/api/mcp-catalog",
];

const TEAM_PREFIXES = ["/api/workbench/"];

/** Already scoped: hub, team, or naming an agent. */
const SCOPED_PREFIXES = ["/api/hub/", "/api/team/", "/api/agents/"];

function startsWithSegment(path: string, prefix: string): boolean {
  if (!path.startsWith(prefix)) return false;
  if (prefix.endsWith("/")) return true;
  const next = path.charAt(prefix.length);
  return next === "" || next === "/" || next === "?";
}

/** Whether a `/api/checkpoints...` query names a hub-level repo (`hub` or `team`). */
function isHubCheckpointRepo(path: string): boolean {
  const query = path.split("?")[1] ?? "";
  const repo = new URLSearchParams(query).get("repo");
  return repo === "hub" || repo === "team";
}

/**
 * Map an unscoped `/api/...` path (with optional query) to its scoped
 * contract path. Already-scoped paths pass through. A path that belongs to an
 * agent resolves to `agent`, and throws `NoAgentSelectedError` when there is none.
 */
export function scopeApiPath(path: string, agent: string | null): string {
  if (SCOPED_PREFIXES.some((p) => path.startsWith(p))) return path;
  if (HUB_PREFIXES.some((p) => startsWithSegment(path, p))) {
    return hubPath(path.slice("/api".length));
  }
  if (startsWithSegment(path, "/api/checkpoints") && isHubCheckpointRepo(path)) {
    return hubPath(path.slice("/api".length));
  }
  if (TEAM_PREFIXES.some((p) => startsWithSegment(path, p))) {
    return teamPath(path.slice("/api".length));
  }
  return agentPath(requireAgent(agent), path.slice("/api".length));
}
