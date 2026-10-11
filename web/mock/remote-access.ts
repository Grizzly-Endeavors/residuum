import type { RemoteAccessStatus } from "../src/lib/generated/RemoteAccessStatus";
import { json, readJsonObject, stringField } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";

/** What the hub knows about the secure tunnel: its status and a recovery code not yet saved. */
export interface MockRemoteAccess {
  status: RemoteAccessStatus;
}

/** An install without Residuum Cloud set up, where the remote access group stays hidden. */
export function defaultRemoteAccess(): MockRemoteAccess {
  return {
    status: {
      state: "disabled",
      detail: "Residuum Cloud isn't set up on this install.",
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
          removable: false,
        },
      ],
      recovery_code_pending: false,
      recovery_code: null,
      instances: [],
      siblings: [],
      join: null,
      pending_joins: [],
      pending_reset: null,
    },
  };
}

/**
 * An install on the secure tunnel with the user's other instances around it:
 * two instances (the desktop is active, this laptop is not), one instance
 * asking to join, a join this one started and is waiting on, a joined
 * sibling, and a certificate account whose instance is gone.
 */
export function clusterRemoteAccess(): MockRemoteAccess {
  const base = defaultRemoteAccess().status;
  return {
    status: {
      ...base,
      state: "ready",
      detail: null,
      pins: [
        {
          account_uri: "https://acme-v02.api.letsencrypt.org/acme/acct/1",
          slug: "laptop",
          own: true,
          known: true,
          removable: false,
        },
        {
          account_uri: "https://acme-v02.api.letsencrypt.org/acme/acct/2",
          slug: "desktop",
          own: false,
          known: true,
          removable: false,
        },
        {
          account_uri: "https://acme-v02.api.letsencrypt.org/acme/acct/3",
          slug: "old-box",
          own: false,
          known: true,
          removable: true,
        },
      ],
      instances: [
        { slug: "laptop", display_name: "Laptop", active: false, connected: true },
        { slug: "desktop", display_name: "Desktop", active: true, connected: true },
      ],
      siblings: [{ slug: "desktop", display_name: "Desktop" }],
      join: { instance: "desktop", state: "waiting", code: "482916", detail: null },
      pending_joins: [
        {
          id: "join-1",
          code: "731504",
          slug: "tablet",
          display_name: "Tablet",
          in_relay_list: false,
          expires_at: "2026-12-01T00:10:00Z",
        },
      ],
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

/** `POST /api/hub/remote-access/email-reset`: the reset link is "sent", and the reset waits for confirmation. */
function emailReset(ctx: RouteContext): void {
  const { status: current } = held(ctx);
  current.pending_reset = {
    slug: current.slug ?? "laptop",
    account_uri: "https://acme-v02.api.letsencrypt.org/acme/acct/1",
    own: true,
    confirmed: false,
    effective_at: null,
    cancellable: false,
  };
  json(ctx.res, 200, { email: "b***@example.com" });
}

/** `POST /api/hub/remote-access/cancel-reset`: only a reset that is waiting can be cancelled. */
function cancelReset(ctx: RouteContext): void {
  const { status: current } = held(ctx);
  if (current.pending_reset === null) {
    json(ctx.res, 400, { error: "No reset is waiting any more." });
    return;
  }
  current.pending_reset = null;
  noContent(ctx);
}

/** `POST /api/hub/remote-access/join`: the other instance is asked, and its code shows while it waits. */
async function join(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req).catch(() => ({}));
  const instance = stringField(body, "instance");
  if (instance === undefined || instance === "") {
    json(ctx.res, 400, { error: "Name the instance to join." });
    return;
  }
  held(ctx).status.join = { instance, state: "waiting", code: "482916", detail: null };
  noContent(ctx);
}

/** `POST /api/hub/remote-access/joins/{id}/approve` and `.../deny`. */
function answerJoin(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const { status: current } = held(ctx);
  if (!current.pending_joins.some((pending) => pending.id === id)) {
    json(ctx.res, 404, { error: "No such join request." });
    return;
  }
  current.pending_joins = current.pending_joins.filter((pending) => pending.id !== id);
  noContent(ctx);
}

/** `POST /api/hub/remote-access/pins/remove`: only a removable account goes. */
async function removeAccount(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req).catch(() => ({}));
  const uri = stringField(body, "account_uri");
  const { status: current } = held(ctx);
  if (!current.pins.some((pin) => pin.account_uri === uri && pin.removable)) {
    json(ctx.res, 400, { error: "That certificate account can't be removed." });
    return;
  }
  current.pins = current.pins.filter((pin) => pin.account_uri !== uri);
  noContent(ctx);
}

/** `POST /api/hub/remote-access/instances/{slug}/activate`. */
function activate(ctx: RouteContext): void {
  const slug = decodedParam(ctx, 0);
  const { status: current } = held(ctx);
  if (!current.instances.some((instance) => instance.slug === slug)) {
    json(ctx.res, 404, { error: "No such instance." });
    return;
  }
  current.instances = current.instances.map((instance) => ({
    ...instance,
    active: instance.slug === slug,
  }));
  noContent(ctx);
}

/** `POST /api/mock/remote-access` with `{ "scenario": "cluster" }`: the secure tunnel with other instances, joins and a removable account. Any other scenario is the older tunnel. */
async function scenario(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req).catch(() => ({}));
  ctx.hub.hubState.remoteAccess =
    stringField(body, "scenario") === "cluster" ? clusterRemoteAccess() : defaultRemoteAccess();
  noContent(ctx);
}

/** The remote access routes, in the unscoped `/api/...` spelling of the hub's `/api/hub/...`. */
export const remoteAccessRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/remote-access/status", handler: status },
  { method: "POST", pattern: "/api/remote-access/retry", handler: noContent },
  { method: "POST", pattern: "/api/remote-access/recovery-code/saved", handler: saved },
  { method: "POST", pattern: "/api/remote-access/reset-pins", handler: reset },
  { method: "POST", pattern: "/api/remote-access/email-reset", handler: emailReset },
  { method: "POST", pattern: "/api/remote-access/cancel-reset", handler: cancelReset },
  { method: "POST", pattern: "/api/remote-access/join", handler: join },
  {
    method: "POST",
    pattern: /^\/api\/remote-access\/joins\/([^/]+)\/approve$/,
    handler: answerJoin,
  },
  {
    method: "POST",
    pattern: /^\/api\/remote-access\/joins\/([^/]+)\/deny$/,
    handler: answerJoin,
  },
  { method: "POST", pattern: "/api/remote-access/pins/remove", handler: removeAccount },
  {
    method: "POST",
    pattern: /^\/api\/remote-access\/instances\/([^/]+)\/activate$/,
    handler: activate,
  },
  { method: "POST", pattern: "/api/mock/remote-access", handler: scenario },
];
