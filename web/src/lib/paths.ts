// ── API base paths ───────────────────────────────────────────────────
//
// Every HTTP and WebSocket URL the app builds goes through here. The
// backend serves three scopes (see docs/design/multi-agent-hub/http-contract.md):
//
//   /api/agents/{name}/...   everything belonging to one agent
//   /api/hub/...             one-per-process things: lifecycle, hub config, secrets
//   /api/team/...            the shared team layer: team files, workbench
//
// The agent scope names the current agent, which the router sets whenever
// the location changes.

const AGENT_STORAGE_KEY = "residuum-last-agent";

let currentAgent: string | null = null;

/** The agent agent-scoped calls address now, or `null` before one is chosen. */
export function getCurrentAgent(): string | null {
  return currentAgent;
}

type AgentListener = (agent: string | null) => void;

const agentListeners = new Set<AgentListener>();

/**
 * Point agent-scoped calls at `name`. Listeners run before this returns, so
 * whatever holds the previous agent's state is torn down before any call can
 * address the new one.
 */
export function setCurrentAgent(name: string | null): void {
  if (name === currentAgent) return;
  currentAgent = name;
  for (const listener of agentListeners) listener(name);
}

/** Observe the current agent changing. Returns a function that stops observing. */
export function onCurrentAgentChange(listener: AgentListener): () => void {
  agentListeners.add(listener);
  return () => agentListeners.delete(listener);
}

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

/** Thrown when an agent-scoped call is made with no agent chosen. */
export class NoAgentSelectedError extends Error {
  constructor() {
    super("no agent is selected");
    this.name = "NoAgentSelectedError";
  }
}

/** `/api/agents/{name}` for `agent`, or the current agent. */
export function agentBase(agent: string | null = currentAgent): string {
  if (agent === null) throw new NoAgentSelectedError();
  return `/api/agents/${encodeURIComponent(agent)}`;
}

/** An agent-scoped API path: `agentPath("/status")` is `/api/agents/{current}/status`. */
export function agentPath(sub: string, agent: string | null = currentAgent): string {
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
 * contract path. Already-scoped paths pass through.
 */
export function scopeApiPath(path: string, agent: string | null = currentAgent): string {
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
  return agentPath(path.slice("/api".length), agent);
}
