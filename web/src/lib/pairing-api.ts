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

export function fetchPairingState(): Promise<PairingStateResponse> {
  return send(hubPath("/pairing/state"));
}

export function createPairingRequest(deviceName: string): Promise<PairingRequestCreated> {
  const body: CreatePairingRequestBody = { device_name: deviceName };
  return send(hubPath("/pairing/requests"), postJson(body));
}

export function pollPairingRequest(requestId: string): Promise<PairingPollResponse> {
  return send(hubPath("/pairing/requests/poll"), postJson({ request_id: requestId }));
}

export function redeemPairingToken(token: string, deviceName: string): Promise<PairedResponse> {
  return send(hubPath("/pairing/redeem"), postJson({ token, device_name: deviceName }));
}

export function redeemRecoveryCode(code: string, deviceName: string): Promise<PairedResponse> {
  return send(hubPath("/pairing/recovery"), postJson({ code, device_name: deviceName }));
}

export function fetchDevices(): Promise<DeviceListResponse> {
  return send(hubPath("/devices"));
}

export function revokeDevice(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/${encodeURIComponent(id)}`), { method: "DELETE" });
}

export function approvePairing(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/pending/${encodeURIComponent(id)}/approve`), {
    method: "POST",
  });
}

export function denyPairing(id: string): Promise<void> {
  return sendEmpty(hubPath(`/devices/pending/${encodeURIComponent(id)}/deny`), {
    method: "POST",
  });
}

export function regenerateRecoveryCodes(): Promise<RecoveryCodesResponse> {
  return send(hubPath("/devices/recovery-codes"), { method: "POST" });
}

export function createPairLink(): Promise<PairLinkResponse> {
  return send(hubPath("/remote-access/pair-link"), { method: "POST" });
}

export function createWorkbenchHandoff(): Promise<WorkbenchHandoffResponse> {
  return send(hubPath("/devices/workbench-handoff"), { method: "POST" });
}
