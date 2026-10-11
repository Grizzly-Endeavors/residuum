import type { UpdateStatusResponse } from "../src/lib/types";
import { MOCK_RESIDUUM_VERSION } from "./constants";
import { json, text } from "./http";
import type { Route, RouteContext } from "./routes";

/** What the hub knows about updates: nothing until a check runs. */
export interface MockUpdateStatus {
  latest: string | null;
  lastChecked: string | null;
}

/** The status the update routes report. The mock is always on the latest version, so no update is ever available. */
function statusOf({ state }: RouteContext): UpdateStatusResponse {
  return {
    current: MOCK_RESIDUUM_VERSION,
    latest: state.update.latest,
    update_available: false,
    last_checked: state.update.lastChecked,
    checking: false,
    rollback_notice: null,
    unverified_update: null,
  };
}

/** `POST /api/hub/update/check`: look for an update now, and answer with the status. */
function checkForUpdate(ctx: RouteContext): void {
  ctx.state.update = { latest: MOCK_RESIDUUM_VERSION, lastChecked: ctx.state.env.clock.iso() };
  json(ctx.res, 200, statusOf(ctx));
}

/** `POST /api/hub/update/apply`: install the version a check found. Refused before any check, as the backend does. */
function applyUpdate(ctx: RouteContext): void {
  if (ctx.state.update.latest === null) {
    text(ctx.res, 400, "no update version known — run a check first");
    return;
  }
  json(ctx.res, 200, statusOf(ctx));
}

/** The update routes, in the unscoped `/api/...` spelling of the hub's `/api/hub/update/...`. */
export const updateRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/update/status",
    handler: (ctx) => {
      json(ctx.res, 200, statusOf(ctx));
    },
  },
  { method: "POST", pattern: "/api/update/check", handler: checkForUpdate },
  { method: "POST", pattern: "/api/update/apply", handler: applyUpdate },
];
