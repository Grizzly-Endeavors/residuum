import type { MockHub, MockState } from "./state";

const REPAIR_ROUTES = /^\/(config|providers|mcp|workspace|checkpoints|teams-setup)(\/|$)/;

const AGENT_ROUTES =
  /^\/(status|config|providers|mcp|workspace|checkpoints|chat|usage|inbox|agent-inbox|sessions|scheduled|files|memory|model|teams-setup|a2a\/(status|card|agents|outbound))(\/|$)/;
const HUB_ROUTES =
  /^\/(secrets|agent-keys|a2a\/keys|cloud|update|tracing|system|system-one|mcp-catalog|providers\/models|shutdown|checkpoints|push|pairing|devices|remote-access)(\/|$)/;
const TEAM_ROUTES = /^\/workbench(\/|$)/;

function notInScope(path: string, owner: string): ScopeRefusal {
  return { status: 404, body: { error: `mock: ${path} is not a route of ${owner}` } };
}

const INBOX_ITEM_ACTIONS = new Set(["read", "archive", "restore"]);

export function isFileDataRoute(sub: string): boolean {
  const [first = "", ...rest] = sub.replace(/^\/+/, "").split("/");
  switch (first) {
    case "chat":
      return rest.length === 1 && rest[0] === "history";
    case "usage":
      return rest.length === 0;
    case "a2a":
      return rest.length === 2 && rest[0] === "agents" && rest[1] === "raw";
    case "inbox":
      switch (rest.length) {
        case 0:
          return true;
        case 1:
          return rest[0] === "archive";
        case 2:
          return INBOX_ITEM_ACTIONS.has(rest[1] ?? "");
        case 3:
          return rest[1] === "attachments";
        case 4:
          return rest[0] === "agent" && rest[2] === "attachments";
        default:
          return false;
      }
    default:
      return false;
  }
}

export interface ScopedRequest {
  state: MockState;
  path: string;
}

export interface ScopeRefusal {
  status: number;
  body: { error: string; state?: string };
}

export function scopeRequest(
  hub: MockHub,
  path: string,
  query: URLSearchParams,
): ScopedRequest | ScopeRefusal {
  if (path.startsWith("/api/mock/")) {
    const target = query.get("agent");
    const agent =
      (target ? hub.agents.get(target) : undefined) ??
      [...hub.agents.values()].find((a) => a.runState === "running");
    return { state: agent?.state ?? hub.hubState, path };
  }

  const agentMatch = /^\/api\/agents\/([^/]+)(\/.*)?$/.exec(path);
  if (agentMatch) {
    const name = decodeURIComponent(agentMatch[1] ?? "");
    const sub = agentMatch[2] ?? "";
    const agent = hub.agents.get(name);
    if (!agent) {
      return { status: 404, body: { error: `no agent named '${name}'` } };
    }
    if (agent.runState !== "running" && !REPAIR_ROUTES.test(sub) && !isFileDataRoute(sub)) {
      return {
        status: 409,
        body: { error: `${name} is ${agent.runState}`, state: agent.runState },
      };
    }
    return AGENT_ROUTES.test(sub)
      ? { state: agent.state, path: `/api${sub}` }
      : notInScope(path, "an agent");
  }

  if (
    path === "/api/hub/status" ||
    path === "/api/hub/stop-all" ||
    path === "/api/hub/agents" ||
    path.startsWith("/api/hub/agents/") ||
    path === "/api/hub/events" ||
    path === "/api/hub/overview" ||
    path.startsWith("/api/hub/mail/") ||
    path === "/api/hub/uploads"
  ) {
    return { state: hub.hubState, path };
  }
  if (path.startsWith("/api/hub/config/")) return { state: hub.hubState, path };
  if (path.startsWith("/api/hub/")) {
    const sub = path.slice("/api/hub".length);
    return HUB_ROUTES.test(sub)
      ? { state: hub.hubState, path: `/api${sub}` }
      : notInScope(path, "the hub");
  }

  if (path.startsWith("/api/team/workspace/")) return { state: hub.hubState, path };
  if (path.startsWith("/api/team/")) {
    const sub = path.slice("/api/team".length);
    return TEAM_ROUTES.test(sub)
      ? { state: hub.hubState, path: `/api${sub}` }
      : notInScope(path, "the team");
  }

  return {
    status: 404,
    body: {
      error: `mock: ${path} is not a hub, team, or agent route; the contract scopes every /api path`,
    },
  };
}

export function isRefusal(scoped: ScopedRequest | ScopeRefusal): scoped is ScopeRefusal {
  return "status" in scoped;
}
