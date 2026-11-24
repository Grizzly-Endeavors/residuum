import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { CloudStatusResponse, CloudTunnelStatus } from "../src/lib/types";
import { json, parseJsonObject, readBody, readJsonObject, text, type JsonObject } from "./http";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Route, RouteContext } from "./routes";
import type { MockHub, MockState } from "./state";

export interface MockCloud {
  tunnel: CloudTunnelStatus | null;
  viaTunnel: boolean;
  signIns: string[];
  signInsStarted: number;
}

const DEFAULT_RELAY_URL = "wss://agent-residuum.com/tunnel/v2/register";

const MOCK_CLOUD_USER = "mock-user";

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

function changeCloudTable(hub: MockHub, change: JsonObject): void {
  const state = hub.hubState;
  const doc = parseToml(state.hubConfigToml);
  state.hubConfigToml = stringifyToml({
    ...doc,
    cloud: { ...cloudTable(state.hubConfigToml), ...change },
  });
  hub.reloadHubConfig();
}

function disconnect(ctx: RouteContext): void {
  if (ctx.state.cloud.viaTunnel) {
    text(ctx.res, 403, REMOTE_REFUSAL);
    return;
  }
  ctx.state.cloud.tunnel = null;
  changeCloudTable(ctx.hub, { enabled: false });
  json(ctx.res, 200, { ok: true });
}

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
  ctx.hub.hubState.cloud = { ...held, tunnel, viaTunnel: via };
  json(ctx.res, 200, { tunnel, viaTunnel: via });
}

async function startSignIn(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req);
  const relayUrl = typeof body.relay_url === "string" ? body.relay_url.trim() : "";
  const port = typeof body.port === "string" && body.port.trim() !== "" ? body.port.trim() : "7700";
  const relay = /^(wss?):\/\/([^/?#\s]+)/i.exec(relayUrl === "" ? DEFAULT_RELAY_URL : relayUrl);
  if (relay === null) {
    json(ctx.res, 400, {
      error:
        "The relay address isn't a ws:// or wss:// address, so there is nowhere to sign in. Check the relay URL under More options.",
    });
    return;
  }
  const [, scheme = "", host = ""] = relay;
  const cloud = ctx.hub.hubState.cloud;
  cloud.signInsStarted += 1;
  const state = `mock-sign-in-${String(cloud.signInsStarted).padStart(4, "0")}`;
  cloud.signIns.push(state);
  const origin = `${scheme.toLowerCase() === "wss" ? "https" : "http"}://${host.toLowerCase()}`;
  const query = new URLSearchParams({ port, state });
  json(ctx.res, 200, { url: `${origin}/connect?${query.toString()}` });
}

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

function saveSignIn(hub: MockHub, token: string): void {
  hub.hubState.secrets.set("cloud_token", token);
  changeCloudTable(hub, { enabled: true, token: "secret:cloud_token" });
}

const CLOUD_SETTINGS_PATH = "/home?settings=_all/cloud";

function redeemSignIn(hub: MockHub, state: string | undefined): boolean {
  const started = hub.hubState.cloud.signIns;
  const index = state === undefined ? -1 : started.indexOf(state);
  if (index === -1) return false;
  started.splice(index, 1);
  return true;
}

export function createCloudCallbackHandler(
  hub: MockHub,
): (req: IncomingMessage, res: ServerResponse) => Promise<boolean> {
  return (req, res) => {
    const [path = "", query = ""] = (req.url ?? "").split("?");
    if (req.method !== "GET" || path !== "/cloud/callback") return Promise.resolve(false);
    const params = new URLSearchParams(query);
    const state = params.getAll("state").find((s) => s !== "");
    const token = params.getAll("token").find((t) => t.trim() !== "");
    let outcome = "not-started-here";
    if (redeemSignIn(hub, state)) {
      outcome = "missing-token";
      if (token !== undefined) {
        saveSignIn(hub, token);
        outcome = "connecting";
      }
    }
    res.writeHead(303, { location: `${CLOUD_SETTINGS_PATH}&cloud=${outcome}` });
    res.end();
    return Promise.resolve(true);
  };
}

export const cloudRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/cloud/status",
    handler: ({ res, state }) => {
      json(res, 200, cloudStatusOf(state));
    },
  },
  { method: "POST", pattern: "/api/cloud/connect", handler: startSignIn },
  { method: "POST", pattern: "/api/cloud/disconnect", handler: disconnect },
  { method: "POST", pattern: "/api/mock/cloud", handler: holdTunnel },
  { method: "POST", pattern: "/api/mock/cloud-callback", handler: relayCallback },
];
