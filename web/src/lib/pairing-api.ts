// ── Device pairing API wrappers ──────────────────────────────────────
//
// Kept apart from `api.ts`: the pairing page loads before anything else in
// the app and uses only these.

import { ApiError } from "./api";
import type { CreatePairingRequestBody } from "./generated/CreatePairingRequestBody";
import type { DeviceListResponse } from "./generated/DeviceListResponse";
import type { PairLinkResponse } from "./generated/PairLinkResponse";
import type { PairedResponse } from "./generated/PairedResponse";
import type { PairingPollResponse } from "./generated/PairingPollResponse";
import type { PairingRequestCreated } from "./generated/PairingRequestCreated";
import type { PairingStateResponse } from "./generated/PairingStateResponse";
import type { RecoveryCodesResponse } from "./generated/RecoveryCodesResponse";
import type { WorkbenchHandoffResponse } from "./generated/WorkbenchHandoffResponse";
import { hubPath } from "./paths";

async function send<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, init);
  if (!resp.ok) throw new ApiError(resp.status, resp.statusText, await resp.text());
  return (await resp.json()) as T;
}

async function sendEmpty(path: string, init: RequestInit): Promise<void> {
  const resp = await fetch(path, init);
  if (!resp.ok) throw new ApiError(resp.status, resp.statusText, await resp.text());
}

function postJson(body: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}

// ── Before pairing ───────────────────────────────────────────────────

/** Whether this browser needs to pair. */
export function fetchPairingState(): Promise<PairingStateResponse> {
  return send(hubPath("/pairing/state"));
}

/** Ask to be paired. The response holds the code to show and the secret to poll with. */
export function createPairingRequest(deviceName: string): Promise<PairingRequestCreated> {
  const body: CreatePairingRequestBody = { device_name: deviceName };
  return send(hubPath("/pairing/requests"), postJson(body));
}

/** Check on a pairing request. An approved one sets the device cookie in this response. */
export function pollPairingRequest(requestId: string): Promise<PairingPollResponse> {
  return send(hubPath("/pairing/requests/poll"), postJson({ request_id: requestId }));
}

/** Pair with a link's single-use token. */
export function redeemPairingToken(token: string, deviceName: string): Promise<PairedResponse> {
  return send(hubPath("/pairing/redeem"), postJson({ token, device_name: deviceName }));
}

/** Pair with a recovery code. */
export function redeemRecoveryCode(code: string, deviceName: string): Promise<PairedResponse> {
  return send(hubPath("/pairing/recovery"), postJson({ code, device_name: deviceName }));
}

// ── Managing paired devices ──────────────────────────────────────────

/** Paired devices and the requests waiting for an answer. */
export function fetchDevices(): Promise<DeviceListResponse> {
  return send(hubPath("/devices"));
}

/** Stop trusting a device. */
export function revokeDevice(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/${encodeURIComponent(id)}`), { method: "DELETE" });
}

/** Approve the waiting request `id`. */
export function approvePairing(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/pending/${encodeURIComponent(id)}/approve`), {
    method: "POST",
  });
}

/** Refuse the waiting request `id`. */
export function denyPairing(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/pending/${encodeURIComponent(id)}/deny`), {
    method: "POST",
  });
}

/** Replace the recovery codes with ten new ones, shown once. */
export function regenerateRecoveryCodes(): Promise<RecoveryCodesResponse> {
  return send(hubPath("/devices/recovery-codes"), { method: "POST" });
}

/** Make the link that pairs the first device. Only the machine running Residuum can. */
export function createPairLink(): Promise<PairLinkResponse> {
  return send(hubPath("/remote-access/pair-link"), { method: "POST" });
}

/** A single-use token that brings a paired browser's credential to the workbench host. */
export function createWorkbenchHandoff(): Promise<WorkbenchHandoffResponse> {
  return send(hubPath("/devices/workbench-handoff"), { method: "POST" });
}
