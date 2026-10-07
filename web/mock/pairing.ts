import type { DeviceInfo } from "../src/lib/generated/DeviceInfo";
import type { DeviceListResponse } from "../src/lib/generated/DeviceListResponse";
import type { PairLinkResponse } from "../src/lib/generated/PairLinkResponse";
import type { PairingStateResponse } from "../src/lib/generated/PairingStateResponse";
import type { PendingPairingInfo } from "../src/lib/generated/PendingPairingInfo";
import type { RecoveryCodesResponse } from "../src/lib/generated/RecoveryCodesResponse";
import { json } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";

/** What the hub knows about remote access: paired browsers, waiting requests, recovery codes. Only the hub's state holds any. */
export interface MockPairing {
  devices: DeviceInfo[];
  pending: PendingPairingInfo[];
  /** Whether recovery codes were ever made: the first pairing link shows them. */
  codesMade: boolean;
  recoveryCodesRemaining: number;
}

/** The address Residuum Cloud announces for the mock. */
const MOCK_UI_ORIGIN = "https://mock-user.agent-residuum.com";

const SAMPLE_CODES = [
  "ABCD-EFGH-IJKL-MNOP",
  "QRST-UVWX-YZ23-4567",
  "BCDE-FGHJ-KLMN-PQRS",
  "TUVW-XYZ2-3456-7ABC",
  "DEFG-HIJK-LMNO-PQRS",
  "TUVW-XYZ2-3456-7ABD",
  "EFGH-IJKL-MNOP-QRST",
  "UVWX-YZ23-4567-ABCE",
  "FGHI-JKLM-NOPQ-RSTU",
  "VWXY-Z234-567A-BCDF",
];

/** A QR code stand-in: a real SVG document, not a scannable one. */
const SAMPLE_QR =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8" fill="#fff"/><path d="M1 1h2v2H1zM5 1h2v2H5zM1 5h2v2H1zM4 4h1v1H4zM6 6h1v1H6z"/></svg>';

export function emptyPairing(): MockPairing {
  return { devices: [], pending: [], codesMade: false, recoveryCodesRemaining: 0 };
}

function pairing(ctx: RouteContext): MockPairing {
  return ctx.state.pairing;
}

/** `GET /api/hub/pairing/state`: the mock is always served locally, which needs no pairing. */
function state(ctx: RouteContext): void {
  json(ctx.res, 200, { remote: false, paired: true } satisfies PairingStateResponse);
}

/** `GET /api/hub/devices`. */
function list(ctx: RouteContext): void {
  const { devices, pending, recoveryCodesRemaining } = pairing(ctx);
  json(ctx.res, 200, {
    devices,
    pending,
    recovery_codes_remaining: recoveryCodesRemaining,
    ui_origin: MOCK_UI_ORIGIN,
    remote: false,
  } satisfies DeviceListResponse);
}

/** `DELETE /api/hub/devices/{id}`. */
function revoke(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const held = pairing(ctx);
  const before = held.devices.length;
  held.devices = held.devices.filter((device) => device.id !== id);
  if (held.devices.length === before) {
    json(ctx.res, 404, { error: "There's no paired device with that id." });
    return;
  }
  ctx.res.writeHead(204);
  ctx.res.end();
}

/** `POST /api/hub/devices/pending/{id}/approve` and `/deny`. */
function answer(approve: boolean): (ctx: RouteContext) => void {
  return (ctx) => {
    const id = decodedParam(ctx, 0);
    const held = pairing(ctx);
    const request = held.pending.find((entry) => entry.id === id);
    if (request === undefined) {
      json(ctx.res, 404, { error: "That pairing request is gone." });
      return;
    }
    held.pending = held.pending.filter((entry) => entry.id !== id);
    if (approve) {
      const now = ctx.state.env.clock.iso();
      held.devices.push({
        id: `device-${String(held.devices.length + 1)}`,
        name: request.device_name,
        created_at: now,
        last_seen: now,
        current: false,
      });
    }
    ctx.res.writeHead(204);
    ctx.res.end();
  };
}

/** `POST /api/hub/devices/recovery-codes`. */
function recoveryCodes(ctx: RouteContext): void {
  const held = pairing(ctx);
  held.codesMade = true;
  held.recoveryCodesRemaining = SAMPLE_CODES.length;
  json(ctx.res, 200, { recovery_codes: SAMPLE_CODES } satisfies RecoveryCodesResponse);
}

/** `POST /api/hub/remote-access/pair-link`: pairs the first browser, as the machine Residuum runs on. */
function pairLink(ctx: RouteContext): void {
  const held = pairing(ctx);
  const first = !held.codesMade;
  held.codesMade = true;
  if (first) held.recoveryCodesRemaining = SAMPLE_CODES.length;
  json(ctx.res, 200, {
    link: `${MOCK_UI_ORIGIN}/pair#token=mock-token`,
    qr_svg: SAMPLE_QR,
    expires_in_secs: 600,
    recovery_codes: first ? SAMPLE_CODES : null,
  } satisfies PairLinkResponse);
}

/** `POST /api/hub/devices/workbench-handoff`: the mock is served locally, where no handoff is needed. */
function workbenchHandoff(ctx: RouteContext): void {
  json(ctx.res, 409, {
    error:
      "This browser isn't a paired device. Opened on the machine Residuum runs on, the workbench needs no handoff.",
  });
}

/** The pairing routes, in the unscoped `/api/...` spelling of the hub's `/api/hub/...`. */
export const pairingRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/pairing/state", handler: state },
  { method: "GET", pattern: "/api/devices", handler: list },
  { method: "DELETE", pattern: /^\/api\/devices\/([^/]+)$/, handler: revoke },
  { method: "POST", pattern: /^\/api\/devices\/pending\/([^/]+)\/approve$/, handler: answer(true) },
  { method: "POST", pattern: /^\/api\/devices\/pending\/([^/]+)\/deny$/, handler: answer(false) },
  { method: "POST", pattern: "/api/devices/recovery-codes", handler: recoveryCodes },
  { method: "POST", pattern: "/api/devices/workbench-handoff", handler: workbenchHandoff },
  { method: "POST", pattern: "/api/remote-access/pair-link", handler: pairLink },
];
