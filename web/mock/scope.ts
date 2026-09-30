import type { MockHub, MockState } from "./state";

/**
 * Routes that still work on a stopped or failed agent, so the user can repair
 * it: the same list as the backend's repair router (`src/hub/http/dispatch.rs`).
 * The path is the agent's own, below `/api/agents/{name}`.
 */
const REPAIR_ROUTES = /^\/(config|providers|mcp|workspace|checkpoints)(\/|$)/;

const INBOX_ITEM_ACTIONS = new Set(["read", "archive", "restore"]);

/**
 * Whether `sub`, the agent's own path below `/api/agents/{name}`, is one of
 * the file-only routes the backend serves for a stopped or failed agent
 * (`is_file_data_route` in `src/hub/http/dispatch.rs`): chat history, usage,
 * the user inbox, and the raw A2A client settings. They are matched whole,
 * because most routes that share a first segment with one need the agent: of
 * `a2a/...` only `a2a/agents/raw` is a file route.
 */
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
      // The inbox, the archive, `{id}/(read|archive|restore)`, `{id}/attachments/{index}`.
      switch (rest.length) {
        case 0:
          return true;
        case 1:
          return rest[0] === "archive";
        case 2:
          return INBOX_ITEM_ACTIONS.has(rest[1] ?? "");
        case 3:
          return rest[1] === "attachments";
        default:
          return false;
      }
    default:
      return false;
  }
}

/** A request resolved to the state its handlers run against, and the unscoped path they match. */
export interface ScopedRequest {
  state: MockState;
  path: string;
}

/** A request answered without reaching a handler. */
export interface ScopeRefusal {
  status: number;
  body: { error: string; state?: string };
}

/**
 * Resolve a request to the state and unscoped path the route tables expect,
 * or refuse it. The unscoped `/api/...` routes don't exist: only the
 * contract's scoped routes are served, so a call that skips the scope fails
 * here the way it would against the real backend.
 *
 * - `/api/agents/{name}/...` runs against that agent's state, as `/api/...`.
 *   A stopped or failed agent serves only the repair and file-only routes;
 *   any other route answers `409`. An unknown agent answers `404`.
 * - Hub routes run against the shared state. Lifecycle, status and hub config
 *   routes and team files keep their path; any other becomes `/api/...`.
 * - `/api/team/...` likewise, except team files.
 * - `/api/mock/...` test controls run against the `?agent=` agent, or the
 *   first running one.
 */
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
    return { state: agent.state, path: `/api${sub}` };
  }

  if (
    path === "/api/hub/status" ||
    path === "/api/hub/stop-all" ||
    path === "/api/hub/agents" ||
    path.startsWith("/api/hub/agents/")
  ) {
    return { state: hub.hubState, path };
  }
  // Hub config keeps its path; every other hub route is a hub-level route.
  if (path.startsWith("/api/hub/config/")) return { state: hub.hubState, path };
  if (path.startsWith("/api/hub/")) {
    return { state: hub.hubState, path: `/api${path.slice("/api/hub".length)}` };
  }

  if (path.startsWith("/api/team/workspace/")) return { state: hub.hubState, path };
  if (path.startsWith("/api/team/")) {
    return { state: hub.hubState, path: `/api${path.slice("/api/team".length)}` };
  }

  return {
    status: 404,
    body: {
      error: `mock: ${path} is not a hub, team, or agent route; the contract scopes every /api path`,
    },
  };
}

/** Whether `scoped` is a refusal rather than a resolved request. */
export function isRefusal(scoped: ScopedRequest | ScopeRefusal): scoped is ScopeRefusal {
  return "status" in scoped;
}
