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
