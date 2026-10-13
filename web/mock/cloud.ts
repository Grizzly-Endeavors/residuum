import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { CloudStatusResponse, CloudTunnelStatus } from "../src/lib/types";
import { json, parseJsonObject, readBody, readJsonObject, text, type JsonObject } from "./http";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Route, RouteContext } from "./routes";
import type { MockHub, MockState } from "./state";

/** What a test holds the tunnel at, over what the hub's config implies. */
export interface MockCloud {
  /** The tunnel's state while a test holds it. Null follows the config: connected while `[cloud]` has a token and is switched on. */
  tunnel: CloudTunnelStatus | null;
  /** The status is read through the tunnel, so the gateway refuses to disconnect (`remote_control_guard`). */
  viaTunnel: boolean;
}

/** Who the tunnel reports as signed in. */
const MOCK_CLOUD_USER = "mock-user";

/** The guard's refusal, word for word (`src/gateway/remote_control_guard.rs`). */
const REMOTE_REFUSAL =
  "Shutting down or disconnecting can't be done remotely, because nothing could bring Residuum back. Do it on the machine running Residuum.";

function isTunnelStatus(value: unknown): value is CloudTunnelStatus {
  return (
    value === "connecting" ||
    value === "connected" ||
    value === "refused" ||
    value === "disconnected"
  );
}

/** The hub config's `[cloud]` table as the status route reads it; null when there is none. */
function cloudTable(hubConfig: string): JsonObject | null {
  try {
    const { cloud }: JsonObject = parseToml(hubConfig);
    return typeof cloud === "object" && cloud !== null && !Array.isArray(cloud)
      ? (cloud as JsonObject)
      : null;
  } catch {
    return null;
  }
}

/** `GET /api/hub/cloud/status`, read from the hub's config the way the backend reads it (`parse_cloud_state`). */
export function cloudStatusOf(hub: MockState): CloudStatusResponse {
  const cloud = cloudTable(hub.hubConfigToml);
  const hasToken = typeof cloud?.token === "string" && cloud.token.trim() !== "";
  const enabled = cloud === null ? false : cloud.enabled !== false;
  const status = hub.cloud.tunnel ?? (hasToken && enabled ? "connected" : "disconnected");
  return {
    status,
    user_id: status === "connected" ? MOCK_CLOUD_USER : null,
    has_token: hasToken,
    enabled,
    viewed_via_tunnel: hub.cloud.viaTunnel,
  };
}

/** Write keys into the hub config's `[cloud]` table and reload the hub, as the backend does for its own changes to it. */
function changeCloudTable(hub: MockHub, change: JsonObject): void {
  const state = hub.hubState;
  const doc = parseToml(state.hubConfigToml);
  state.hubConfigToml = stringifyToml({
    ...doc,
    cloud: { ...cloudTable(state.hubConfigToml), ...change },
  });
  hub.reloadHubConfig();
}

/** `POST /api/hub/cloud/disconnect`: switch the tunnel off and keep the token. Refused for a request that came through the tunnel. */
function disconnect(ctx: RouteContext): void {
  if (ctx.state.cloud.viaTunnel) {
    text(ctx.res, 403, REMOTE_REFUSAL);
    return;
  }
  ctx.state.cloud.tunnel = null;
  changeCloudTable(ctx.hub, { enabled: false });
  json(ctx.res, 200, { ok: true });
}

/**
 * `{ tunnel?, via_tunnel? }`: hold the tunnel at `connecting`, `connected`,
 * `refused` or `disconnected` (`null` hands it back to the config), and make
 * the status read as served through the tunnel or not.
 */
async function holdTunnel(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req);
  const held = ctx.hub.hubState.cloud;
  const tunnel = "tunnel" in body ? body.tunnel : held.tunnel;
  const via = "via_tunnel" in body ? body.via_tunnel : held.viaTunnel;
  if ((tunnel !== null && !isTunnelStatus(tunnel)) || typeof via !== "boolean") {
    json(ctx.res, 422, {
      error:
        "mock: `tunnel` must be connecting, connected, refused, disconnected or null, and `via_tunnel` a boolean",
    });
    return;
  }
  ctx.hub.hubState.cloud = { tunnel, viaTunnel: via };
  json(ctx.res, 200, ctx.hub.hubState.cloud);
}

/**
 * `{ token? }`: the relay hands a signed-in browser back to the gateway's
 * `/cloud/callback`: the token is kept as the `cloud_token` secret, `[cloud]`
 * is switched on with a reference to it, and the hub reloads.
 */
async function relayCallback(ctx: RouteContext): Promise<void> {
  const raw = await readBody(ctx.req);
  const { token = "rst_mock" } = raw.trim() === "" ? {} : parseJsonObject(raw);
  if (typeof token !== "string" || token === "") {
    json(ctx.res, 422, { error: "mock: `token` must be a non-empty string" });
    return;
  }
  saveSignIn(ctx.hub, token);
  json(ctx.res, 200, { ok: true });
}

/** What the gateway's callback does with a token: keep it as the `cloud_token` secret and switch `[cloud]` on. */
function saveSignIn(hub: MockHub, token: string): void {
  hub.hubState.secrets.set("cloud_token", token);
  changeCloudTable(hub, { enabled: true, token: "secret:cloud_token" });
}

/** Where the gateway's callback sends the browser (`CLOUD_SETTINGS_PATH` in `src/gateway/web/cloud.rs`). */
const CLOUD_SETTINGS_PATH = "/home?settings=_all/cloud";

/**
 * `GET /cloud/callback?token=…`, as the gateway answers it: the first
 * non-empty `token` is saved and the browser is sent to Settings → Residuum
 * Cloud with `cloud=connecting`, or with `cloud=missing-token` when there is
 * none. Answers `false` for any other request, without touching it.
 */
export function createCloudCallbackHandler(
  hub: MockHub,
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  return (req, res) => {
    const [path = "", query = ""] = (req.url ?? "").split("?");
    if (req.method !== "GET" || path !== "/cloud/callback") return Promise.resolve(false);
    const token = new URLSearchParams(query).getAll("token").find((t) => t.trim() !== "");
    let outcome = "missing-token";
    if (token !== undefined) {
      saveSignIn(hub, token);
      outcome = "connecting";
    }
    res.writeHead(303, { location: `${CLOUD_SETTINGS_PATH}&cloud=${outcome}` });
    res.end();
    return Promise.resolve(true);
  };
}

/** The Residuum Cloud routes and the controls that stage its states. */
export const cloudRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/cloud/status",
    handler: ({ res, state }) => {
      json(res, 200, cloudStatusOf(state));
    },
  },
  { method: "POST", pattern: "/api/cloud/disconnect", handler: disconnect },
  { method: "POST", pattern: "/api/mock/cloud", handler: holdTunnel },
  { method: "POST", pattern: "/api/mock/cloud-callback", handler: relayCallback },
];
