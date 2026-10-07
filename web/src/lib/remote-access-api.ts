// ── Remote access API wrappers ───────────────────────────────────────
//
// The secure tunnel's status and the actions on it. Kept apart from `api.ts`
// like the pairing wrappers: they belong to one settings group.

import { ApiError } from "./api";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
import { isInstanceSlug } from "./instance-slug";
import { hubPath } from "./paths";

async function send(path: string, init?: RequestInit): Promise<Response> {
  const resp = await fetch(path, init);
  if (!resp.ok) throw new ApiError(resp.status, resp.statusText, await resp.text());
  return resp;
}

/** Where remote access stands. The recovery code is included only on the machine Residuum runs on. */
export async function fetchRemoteAccess(): Promise<RemoteAccessStatus> {
  return (await (await send(hubPath("/remote-access/status"))).json()) as RemoteAccessStatus;
}

/** Look again now instead of waiting for the next scheduled check. */
export async function retryRemoteAccess(): Promise<void> {
  await send(hubPath("/remote-access/retry"), { method: "POST" });
}

/** The recovery code is saved; Residuum forgets it. */
export async function acknowledgeRecoveryCode(): Promise<void> {
  await send(hubPath("/remote-access/recovery-code/saved"), { method: "POST" });
}

/** Take the address back with the recovery code, pinning this instance's account. */
export async function resetPins(recoveryCode: string): Promise<void> {
  await send(hubPath("/remote-access/reset-pins"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ recovery_code: recoveryCode }),
  });
}

async function sendJson(path: string, body: unknown): Promise<void> {
  await send(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** Ask the existing instance `instance` to approve this one. Progress shows in the status. */
export async function startJoin(instance: string): Promise<void> {
  await sendJson(hubPath("/remote-access/join"), { instance });
}

/** Approve another instance's join request, after its six digits matched. */
export async function approveJoin(id: string): Promise<void> {
  await send(hubPath(`/remote-access/joins/${encodeURIComponent(id)}/approve`), {
    method: "POST",
  });
}

/** Refuse another instance's join request. */
export async function denyJoin(id: string): Promise<void> {
  await send(hubPath(`/remote-access/joins/${encodeURIComponent(id)}/deny`), { method: "POST" });
}

/** Remove a certificate account the status marks removable. */
export async function removePin(accountUri: string): Promise<void> {
  await sendJson(hubPath("/remote-access/pins/remove"), { account_uri: accountUri });
}

/** Make `slug` the instance the user's address goes to. The slug must pass `isInstanceSlug`. */
export async function activateInstance(slug: string): Promise<void> {
  if (!isInstanceSlug(slug)) throw new Error("Not an instance slug.");
  await send(hubPath(`/remote-access/instances/${encodeURIComponent(slug)}/activate`), {
    method: "POST",
  });
}
