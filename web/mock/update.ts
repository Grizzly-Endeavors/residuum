import type { UpdateStatusResponse } from "../src/lib/types";
import { MOCK_RESIDUUM_VERSION } from "./constants";
import { json, text } from "./http";
import type { Route, RouteContext } from "./routes";

export interface MockUpdateStatus {
  latest: string | null;
  lastChecked: string | null;
}

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

function checkForUpdate(ctx: RouteContext): void {
  ctx.state.update = { latest: MOCK_RESIDUUM_VERSION, lastChecked: ctx.state.env.clock.iso() };
  json(ctx.res, 200, statusOf(ctx));
}

function applyUpdate(ctx: RouteContext): void {
  if (ctx.state.update.latest === null) {
    text(ctx.res, 400, "no update version known — run a check first");
    return;
  }
  json(ctx.res, 200, statusOf(ctx));
}

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
