// ── Remote access API wrappers ───────────────────────────────────────
//
// The secure tunnel's status and the actions on it. Kept apart from `api.ts`
// like the pairing wrappers: they belong to one settings group.

import { ApiError } from "./api";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
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
