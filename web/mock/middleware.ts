import type { IncomingMessage, ServerResponse } from "node:http";
import { MockResetError } from "./env";
import { json } from "./http";
import { dispatchRoute, type Route, type RouteRequest } from "./routes";
import { isRefusal, scopeRequest } from "./scope";
import type { MockHub } from "./state";

export interface ApiHandlerOptions {
  hub: MockHub;
  routes: readonly Route[];
}

export function createApiHandler(
  options: ApiHandlerOptions,
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  const { hub, routes } = options;
  return async (req, res) => {
    const url = req.url ?? "";
    const method = req.method ?? "GET";
    if (!url.startsWith("/api")) return false;

    const [rawPath = "", rawQuery = ""] = url.split("?");
    const query = new URLSearchParams(rawQuery);

    try {
      const scoped = scopeRequest(hub, rawPath, query);
      if (isRefusal(scoped)) {
        json(res, scoped.status, scoped.body);
        return true;
      }
      const request: RouteRequest = { req, res, hub, method, query, ...scoped };
      if (await dispatchRoute(routes, request)) return true;
      json(res, 404, { error: `mock: unknown endpoint ${method} ${scoped.path}` });
    } catch (err) {
      if (err instanceof MockResetError) {
        json(res, 503, { error: err.message });
        return true;
      }
      const message = err instanceof Error ? err.message : String(err);
      json(res, 500, { error: `mock server error: ${message}` });
    }
    return true;
  };
}

export function firstToHandle(
  ...handlers: ((req: IncomingMessage, res: ServerResponse) => Promise<boolean>)[]
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  return async (req, res) => {
    for (const handler of handlers) {
      if (await handler(req, res)) return true;
    }
    return false;
  };
}

export function apiMiddleware(
  handler: (req: IncomingMessage, res: ServerResponse) => Promise<boolean>,
): (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void) => void {
  return (req, res, next) => {
    handler(req, res)
      .then((handled) => {
        if (!handled) next();
      })
      .catch((err: unknown) => {
        next(err);
      });
  };
}
