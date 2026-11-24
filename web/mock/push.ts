import type { PatchPushDeviceRequest } from "../src/lib/generated/PatchPushDeviceRequest";
import type { PushDevice } from "../src/lib/generated/PushDevice";
import type { PushDeviceList } from "../src/lib/generated/PushDeviceList";
import type { PushDeviceResponse } from "../src/lib/generated/PushDeviceResponse";
import type { PushKeyResponse } from "../src/lib/generated/PushKeyResponse";
import type { PushPreferences } from "../src/lib/generated/PushPreferences";
import type { PushPreferencesPatch } from "../src/lib/generated/PushPreferencesPatch";
import type { PushTestResult } from "../src/lib/generated/PushTestResult";
import type { PutPushDeviceRequest } from "../src/lib/generated/PutPushDeviceRequest";
import type { WebPushSubscription } from "../src/lib/generated/WebPushSubscription";
import { json, readJsonObject, type JsonObject } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";

export interface MockPushDevice {
  device: PushDevice;
  endpoint: string;
}

export const MOCK_VAPID_PUBLIC_KEY =
  "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";

const UNNAMED_DEVICE = "Unnamed device";

function defaultPreferences(): PushPreferences {
  return {
    inbox_item: true,
    agent_failed: true,
    outbound_unreachable: false,
    reply_while_away: false,
  };
}

const PREFERENCE_NAMES = [
  "inbox_item",
  "agent_failed",
  "outbound_unreachable",
  "reply_while_away",
] as const;

class BadRequest extends Error {}

function badBody(reason: string): BadRequest {
  return new BadRequest(`the request body isn't valid for this route: ${reason}`);
}

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function base64urlBytes(text: string): Buffer | null {
  return /^[A-Za-z0-9_-]+={0,2}$/.test(text) ? Buffer.from(text, "base64url") : null;
}

function parseSubscription(value: unknown): WebPushSubscription {
  if (!isObject(value)) throw badBody("missing field `subscription`");
  const { endpoint, keys } = value;
  if (typeof endpoint !== "string") throw badBody("missing field `endpoint`");
  if (!isObject(keys)) throw badBody("missing field `keys`");
  const { p256dh, auth } = keys;
  if (typeof p256dh !== "string") throw badBody("missing field `p256dh`");
  if (typeof auth !== "string") throw badBody("missing field `auth`");

  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    throw new BadRequest("the subscription's endpoint isn't a web address (invalid URL)");
  }
  if (url.protocol !== "https:") {
    throw new BadRequest("the subscription's endpoint must be an https:// address");
  }
  const point = base64urlBytes(p256dh);
  if (point?.length !== 65 || point[0] !== 0x04) {
    throw new BadRequest(
      "the subscription's p256dh key isn't a base64url P-256 public key (65 bytes starting with 0x04)",
    );
  }
  if (base64urlBytes(auth)?.length !== 16) {
    throw new BadRequest("the subscription's auth secret isn't 16 base64url-encoded bytes");
  }
  return { endpoint, keys: { p256dh, auth } };
}

function parseLabel(value: unknown): string | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") throw badBody("invalid type for `label`: expected a string");
  const label = value.trim();
  if (label === "") throw new BadRequest("a device needs a name");
  return label;
}

function parsePreferencesPatch(value: unknown): PushPreferencesPatch | undefined {
  if (value === undefined || value === null) return undefined;
  if (!isObject(value)) throw badBody("invalid type for `preferences`: expected a map");
  const patch: PushPreferencesPatch = {};
  for (const name of PREFERENCE_NAMES) {
    const setting = value[name];
    if (setting === undefined || setting === null) continue;
    if (typeof setting !== "boolean") {
      throw badBody(`invalid type for \`${name}\`: expected a boolean`);
    }
    patch[name] = setting;
  }
  return patch;
}

function patched(
  current: PushPreferences,
  patch: PushPreferencesPatch | undefined,
): PushPreferences {
  return { ...current, ...patch };
}

async function bodyOr400(ctx: RouteContext): Promise<JsonObject | null> {
  try {
    return await readJsonObject(ctx.req);
  } catch (err) {
    json(ctx.res, 400, {
      error: badBody(err instanceof Error ? err.message : String(err)).message,
    });
    return null;
  }
}

async function refusing(ctx: RouteContext, handle: () => Promise<void> | void): Promise<void> {
  try {
    await handle();
  } catch (err) {
    if (!(err instanceof BadRequest)) throw err;
    json(ctx.res, 400, { error: err.message });
  }
}

function namedDevice(ctx: RouteContext): MockPushDevice | null {
  const id = decodedParam(ctx, 0);
  const found = ctx.state.pushDevices.find((entry) => entry.device.id === id);
  if (found === undefined) {
    json(ctx.res, 404, { error: `there is no notification device with id '${id}'` });
    return null;
  }
  return found;
}

function publicKey(ctx: RouteContext): void {
  json(ctx.res, 200, { public_key: MOCK_VAPID_PUBLIC_KEY } satisfies PushKeyResponse);
}

function listDevices(ctx: RouteContext): void {
  json(ctx.res, 200, {
    devices: ctx.state.pushDevices.map((entry) => entry.device),
  } satisfies PushDeviceList);
}

async function putDevice(ctx: RouteContext): Promise<void> {
  await refusing(ctx, async () => {
    const body = await bodyOr400(ctx);
    if (body === null) return;
    const request: PutPushDeviceRequest = {
      subscription: parseSubscription(body.subscription),
      label: parseLabel(body.label),
      preferences: parsePreferencesPatch(body.preferences),
    };
    const { pushDevices, env } = ctx.state;
    let entry = pushDevices.find((e) => e.endpoint === request.subscription.endpoint);
    entry ??= {
      endpoint: request.subscription.endpoint,
      device: {
        id: `push-device-${String(env.nextId())}`,
        label: UNNAMED_DEVICE,
        created_at: env.clock.iso(),
        last_success_at: null,
        last_failure: null,
        preferences: defaultPreferences(),
      },
    };
    if (!pushDevices.includes(entry)) pushDevices.push(entry);
    if (request.label !== undefined) entry.device.label = request.label;
    entry.device.preferences = patched(entry.device.preferences, request.preferences);
    json(ctx.res, 200, { device: entry.device } satisfies PushDeviceResponse);
  });
}

async function patchDevice(ctx: RouteContext): Promise<void> {
  await refusing(ctx, async () => {
    const body = await bodyOr400(ctx);
    if (body === null) return;
    const request: PatchPushDeviceRequest = {
      label: parseLabel(body.label),
      preferences: parsePreferencesPatch(body.preferences),
    };
    if (request.label === undefined && request.preferences === undefined) {
      throw new BadRequest("the request must set a label or preferences");
    }
    const entry = namedDevice(ctx);
    if (entry === null) return;
    if (request.label !== undefined) entry.device.label = request.label;
    entry.device.preferences = patched(entry.device.preferences, request.preferences);
    json(ctx.res, 200, { device: entry.device } satisfies PushDeviceResponse);
  });
}

function deleteDevice(ctx: RouteContext): void {
  const entry = namedDevice(ctx);
  if (entry === null) return;
  ctx.state.pushDevices.splice(ctx.state.pushDevices.indexOf(entry), 1);
  ctx.res.writeHead(204);
  ctx.res.end();
}

function testDevice(ctx: RouteContext): void {
  const entry = namedDevice(ctx);
  if (entry === null) return;
  entry.device.last_success_at = ctx.state.env.clock.iso();
  json(ctx.res, 200, { delivered: true, error: null } satisfies PushTestResult);
}

const DEVICE_ROUTE = "^/api/push/devices/([^/]+)";

export const pushRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/push/key", handler: publicKey },
  { method: "GET", pattern: "/api/push/devices", handler: listDevices },
  { method: "PUT", pattern: "/api/push/devices", handler: putDevice },
  { method: "PATCH", pattern: new RegExp(`${DEVICE_ROUTE}$`), handler: patchDevice },
  { method: "DELETE", pattern: new RegExp(`${DEVICE_ROUTE}$`), handler: deleteDevice },
  { method: "POST", pattern: new RegExp(`${DEVICE_ROUTE}/test$`), handler: testDevice },
];
