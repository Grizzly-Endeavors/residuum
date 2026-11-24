import { ApiError } from "./api";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
import { isInstanceSlug } from "./instance-slug";
import { hubPath } from "./paths";

async function send(path: string, init?: RequestInit): Promise<Response> {
  const resp = await fetch(path, init);
  if (!resp.ok) throw new ApiError(resp.status, resp.statusText, await resp.text());
  return resp;
}

export async function fetchRemoteAccess(): Promise<RemoteAccessStatus> {
  return (await (await send(hubPath("/remote-access/status"))).json()) as RemoteAccessStatus;
}

export async function retryRemoteAccess(): Promise<void> {
  await send(hubPath("/remote-access/retry"), { method: "POST" });
}

export async function acknowledgeRecoveryCode(): Promise<void> {
  await send(hubPath("/remote-access/recovery-code/saved"), { method: "POST" });
}

export async function resetPins(recoveryCode: string): Promise<void> {
  await send(hubPath("/remote-access/reset-pins"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ recovery_code: recoveryCode }),
  });
}

export async function requestEmailReset(): Promise<string> {
  const resp = await send(hubPath("/remote-access/email-reset"), { method: "POST" });
  return ((await resp.json()) as { email: string }).email;
}

export async function cancelPendingReset(): Promise<void> {
  await send(hubPath("/remote-access/cancel-reset"), { method: "POST" });
}

async function sendJson(path: string, body: unknown): Promise<void> {
  await send(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function startJoin(instance: string): Promise<void> {
  await sendJson(hubPath("/remote-access/join"), { instance });
}

export async function approveJoin(id: string): Promise<void> {
  await send(hubPath(`/remote-access/joins/${encodeURIComponent(id)}/approve`), {
    method: "POST",
  });
}

export async function denyJoin(id: string): Promise<void> {
  await send(hubPath(`/remote-access/joins/${encodeURIComponent(id)}/deny`), { method: "POST" });
}

export async function removePin(accountUri: string): Promise<void> {
  await sendJson(hubPath("/remote-access/pins/remove"), { account_uri: accountUri });
}

export async function activateInstance(slug: string): Promise<void> {
  if (!isInstanceSlug(slug)) throw new Error("Not an instance slug.");
  await send(hubPath(`/remote-access/instances/${encodeURIComponent(slug)}/activate`), {
    method: "POST",
  });
}
