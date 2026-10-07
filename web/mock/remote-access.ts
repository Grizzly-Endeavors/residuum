import type { RemoteAccessStatus } from "../src/lib/generated/RemoteAccessStatus";
import { json, readJsonObject, stringField } from "./http";
import type { Route, RouteContext } from "./routes";

/** What the hub knows about the secure tunnel: its status and a recovery code not yet saved. */
export interface MockRemoteAccess {
  status: RemoteAccessStatus;
}

/** An install on the older tunnel, where the remote access group stays hidden. */
export function defaultRemoteAccess(): MockRemoteAccess {
  return {
    status: {
      state: "legacy",
      detail:
        "The relay doesn't offer the secure tunnel yet, so Residuum Cloud uses the older tunnel, which the relay can read.",
      user: "mock-user",
      slug: "laptop",
      hosts: {
        ui: "mock-user.agent-residuum.com",
        workbench: "mock-user.workbench.agent-residuum.com",
        instance: "laptop.mock-user.agent-residuum.com",
      },
      certificate: {
        not_after: "2026-12-01T00:00:00Z",
        renews_at: "2026-11-10T00:00:00Z",
      },
      pins: [
        {
          account_uri: "https://acme-v02.api.letsencrypt.org/acme/acct/1",
          slug: "laptop",
          own: true,
          known: true,
        },
      ],
      recovery_code_pending: false,
      recovery_code: null,
    },
  };
}

function held(ctx: RouteContext): MockRemoteAccess {
  return ctx.state.remoteAccess;
}

/** `GET /api/hub/remote-access/status`: the mock is served locally, so the recovery code is included. */
function status(ctx: RouteContext): void {
  const { status: current } = held(ctx);
  json(ctx.res, 200, {
    ...current,
    recovery_code: current.recovery_code_pending ? "ABCDEFGHIJKLMNOPQRST" : null,
  } satisfies RemoteAccessStatus);
}

function noContent(ctx: RouteContext): void {
  ctx.res.writeHead(204);
  ctx.res.end();
}

/** `POST /api/hub/remote-access/recovery-code/saved`. */
function saved(ctx: RouteContext): void {
  held(ctx).status.recovery_code_pending = false;
  noContent(ctx);
}

/** `POST /api/hub/remote-access/reset-pins`. */
async function reset(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req).catch(() => ({}));
  if (stringField(body, "recovery_code")?.length !== 20) {
    json(ctx.res, 400, { error: "A recovery code is 20 letters and digits (A to Z and 2 to 7)." });
    return;
  }
  held(ctx).status.recovery_code_pending = true;
  noContent(ctx);
}

/** The remote access routes, in the unscoped `/api/...` spelling of the hub's `/api/hub/...`. */
export const remoteAccessRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/remote-access/status", handler: status },
  { method: "POST", pattern: "/api/remote-access/retry", handler: noContent },
  { method: "POST", pattern: "/api/remote-access/recovery-code/saved", handler: saved },
  { method: "POST", pattern: "/api/remote-access/reset-pins", handler: reset },
];
