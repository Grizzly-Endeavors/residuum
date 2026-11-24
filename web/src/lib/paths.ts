const AGENT_STORAGE_KEY = "residuum-last-agent";

export function readLastAgent(): string | null {
  try {
    const value = localStorage.getItem(AGENT_STORAGE_KEY);
    return value !== null && value !== "" ? value : null;
  } catch {
    return null;
  }
}

export function rememberLastAgent(name: string): void {
  try {
    localStorage.setItem(AGENT_STORAGE_KEY, name);
  } catch {
    // localStorage unavailable
  }
}

export class NoAgentSelectedError extends Error {
  constructor() {
    super("no agent is selected");
    this.name = "NoAgentSelectedError";
  }
}

export function requireAgent(agent: string | null): string {
  if (agent === null) throw new NoAgentSelectedError();
  return agent;
}

export function agentBase(agent: string): string {
  return `/api/agents/${encodeURIComponent(agent)}`;
}

export function agentPath(agent: string, sub: string): string {
  return `${agentBase(agent)}${sub}`;
}

export function hubPath(sub: string): string {
  return `/api/hub${sub}`;
}

export function teamPath(sub: string): string {
  return `/api/team${sub}`;
}

function wsUrl(path: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}${path}`;
}

export function agentWsUrl(agent: string): string {
  return wsUrl(`${agentBase(agent)}/ws`);
}

export function agentWatchWsUrl(agent: string): string {
  return `${agentWsUrl(agent)}?watch_only=true`;
}

export function hubWsUrl(): string {
  return wsUrl(hubPath("/ws"));
}
