import type { IncomingMessage, ServerResponse } from "node:http";
import type { MockHub, MockState } from "./state";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

/**
 * What a route handler is given. `path` and every route pattern use the
 * unscoped `/api/...` spelling: scoped requests (`/api/agents/{name}/...`,
 * `/api/hub/...`, `/api/team/...`) are rewritten to it, with `state` set to
 * the agent's or the hub's. The hub's own routes keep their spelling: agent
 * lifecycle and status (`/api/hub/agents...`, `/api/hub/status`), hub config
 * (`/api/hub/config/...`) and team files (`/api/team/workspace/...`).
 */
export interface RouteContext {
  req: IncomingMessage;
  res: ServerResponse;
  hub: MockHub;
  /** The state the request is scoped to: the addressed agent's, or the hub's for hub and team routes. */
  state: MockState;
  method: string;
  path: string;
  query: URLSearchParams;
  /** The capture groups of the matched pattern, still percent-encoded. */
  params: readonly string[];
}

/** A request before a route matches it: what a route handler is given, without `params`. */
export type RouteRequest = Omit<RouteContext, "params">;

/** One endpoint: an exact path, or a pattern whose capture groups become `params`. */
export interface Route {
  method: HttpMethod;
  pattern: string | RegExp;
  handler: (ctx: RouteContext) => void | Promise<void>;
}

/** Run the first route matching the request, and report whether one did. */
export async function dispatchRoute(
  routes: readonly Route[],
  request: RouteRequest,
): Promise<boolean> {
  for (const route of routes) {
    if (route.method !== request.method) continue;
    if (typeof route.pattern === "string") {
      if (route.pattern !== request.path) continue;
      await route.handler({ ...request, params: [] });
      return true;
    }
    const match = route.pattern.exec(request.path);
    if (match === null) continue;
    await route.handler({ ...request, params: match.slice(1) });
    return true;
  }
  return false;
}

/** A capture group of the matched pattern, percent-decoded. */
export function decodedParam(ctx: RouteContext, index: number): string {
  return decodeURIComponent(ctx.params[index] ?? "");
}
