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

/** The first route that takes a request, and the capture groups of its pattern. */
export interface RouteMatch {
  route: Route;
  params: readonly string[];
}

/** The first route taking `method` and `path`, or `undefined` when none does. */
export function matchRoute(
  routes: readonly Route[],
  method: string,
  path: string,
): RouteMatch | undefined {
  for (const route of routes) {
    if (route.method !== method) continue;
    if (typeof route.pattern === "string") {
      if (route.pattern === path) return { route, params: [] };
      continue;
    }
    const match = route.pattern.exec(path);
    if (match !== null) return { route, params: match.slice(1) };
  }
  return undefined;
}

/** Run the first route matching the request, and report whether one did. */
export async function dispatchRoute(
  routes: readonly Route[],
  request: RouteRequest,
): Promise<boolean> {
  const found = matchRoute(routes, request.method, request.path);
  if (found === undefined) return false;
  await found.route.handler({ ...request, params: found.params });
  return true;
}

/** A capture group of the matched pattern, percent-decoded. */
export function decodedParam(ctx: RouteContext, index: number): string {
  return decodeURIComponent(ctx.params[index] ?? "");
}
