// ── Typed fetch wrappers ─────────────────────────────────────────────

import type { StatusResponse, RecentMessage } from "./types";

export async function fetchStatus(): Promise<StatusResponse> {
  const resp = await fetch("/api/status");
  if (!resp.ok) throw new Error(`status check failed: ${resp.status}`);
  return resp.json();
}

export async function fetchChatHistory(): Promise<RecentMessage[]> {
  const resp = await fetch("/api/chat/history");
  if (!resp.ok) return [];
  return resp.json();
}
