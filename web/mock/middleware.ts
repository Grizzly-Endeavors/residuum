import type { IncomingMessage, ServerResponse } from "node:http";
import { json } from "./http";
import { dispatchRoute, type Route, type RouteRequest } from "./routes";
import { isRefusal, scopeRequest } from "./scope";
import type { MockHub } from "./state";

/** What the mock's API handler is built from. */
export interface ApiHandlerOptions {
  hub: MockHub;
  /** The route tables, tried in order. */
  routes: readonly Route[];
  /** Answers a request no route matched, and reports whether it did. */
  fallback?: (request: RouteRequest) => Promise<boolean>;
}

/**
 * The mock's `/api/*` request handler. It scopes the request to an agent or
 * the hub (see `scopeRequest`), runs the first matching route, and answers
 * `404` when nothing matches and `500` when a handler throws. It answers
 * `false` for a request outside `/api`, without touching it.
 */
export function createApiHandler(
  options: ApiHandlerOptions,
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  const { hub, routes, fallback } = options;
  return async (req, res) => {
    const url = req.url ?? "";
    const method = req.method ?? "GET";
    if (!url.startsWith("/api")) return false;

    // Strip the query string for matching, but keep the params for handlers that need them.
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
      if (fallback && (await fallback(request))) return true;
      json(res, 404, { error: `mock: unknown endpoint ${method} ${scoped.path}` });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      json(res, 500, { error: `mock server error: ${message}` });
    }
    return true;
  };
}

/** Connect middleware that runs the API handler, and passes on what it doesn't handle. */
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
