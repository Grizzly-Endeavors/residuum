import type { IncomingMessage, ServerResponse } from "node:http";
import type { MockHub, MockState } from "./state";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface RouteContext {
  req: IncomingMessage;
  res: ServerResponse;
  hub: MockHub;
  state: MockState;
  method: string;
  path: string;
  query: URLSearchParams;
  params: readonly string[];
}

export type RouteRequest = Omit<RouteContext, "params">;

export interface Route {
  method: HttpMethod;
  pattern: string | RegExp;
  handler: (ctx: RouteContext) => void | Promise<void>;
}

export interface RouteMatch {
  route: Route;
  params: readonly string[];
}

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

export async function dispatchRoute(
  routes: readonly Route[],
  request: RouteRequest,
): Promise<boolean> {
  const found = matchRoute(routes, request.method, request.path);
  if (found === undefined) return false;
  await found.route.handler({ ...request, params: found.params });
  return true;
}

export function decodedParam(ctx: RouteContext, index: number): string {
  return decodeURIComponent(ctx.params[index] ?? "");
}
