// ── Hub types ────────────────────────────────────────────────────────
//
// Shapes for the hub API and `/api/hub/ws`. The agent shapes the backend
// exports through ts-rs (`src/hub/types.rs`) are re-exported from
// `generated/`; the rest are hand-written here because the backend builds
// them inline (list and status envelopes, the stop-all body) or does not
// export them (the hub WebSocket's frames).

import type { AgentSummary } from "./generated/AgentSummary";
import type { DeletedAgent } from "./generated/DeletedAgent";
import type { ServerMessage, WorkspaceChange } from "./types";

export type { A2aVisibility } from "./generated/A2aVisibility";
export type { AgentActivity } from "./generated/AgentActivity";
export type { AgentLastError } from "./generated/AgentLastError";
export type { AgentPatch } from "./generated/AgentPatch";
export type { AgentState } from "./generated/AgentState";
export type { AgentSummary } from "./generated/AgentSummary";
export type { CreateAgentRequest } from "./generated/CreateAgentRequest";
export type { DeleteOutcome } from "./generated/DeleteOutcome";
export type { DeletedAgent } from "./generated/DeletedAgent";
export type { RestoreAgentRequest } from "./generated/RestoreAgentRequest";

/** `GET /api/hub/agents`. */
export interface AgentListResponse {
  agents: AgentSummary[];
}

/** `GET /api/hub/agents/deleted`: deleted agents that can be restored, newest deletion first. */
export interface DeletedAgentListResponse {
  agents: DeletedAgent[];
}

/** Who caused a hub event: the user (UI or CLI), or a teammate agent. */
export type HubActor = "user" | `agent:${string}`;

/** `GET /api/hub/status`. */
export interface HubStatusResponse {
  version: string;
  uptime_secs: number;
  /** Same shape as the cloud status response. */
  tunnel: unknown;
  /** How many agents are in each state. */
  agents: { starting: number; running: number; stopped: number; failed: number };
}

/** One agent `POST /api/hub/stop-all` could not stop. */
export interface StopAllFailure {
  name: string;
  error: string;
}

/** `POST /api/hub/stop-all`: `200` when `failed` is empty, `500` with the same body otherwise. */
export interface StopAllResponse {
  stopped: AgentSummary[];
  failed: StopAllFailure[];
}

export type HubNoticeLevel = "info" | "warn" | "error";

/** Server-to-client frames on `/api/hub/ws`. */
export type HubServerMessage =
  | { type: "agents_snapshot"; agents: AgentSummary[] }
  | { type: "agent_state"; agent: AgentSummary }
  | { type: "agent_created"; agent: AgentSummary; by: HubActor }
  | { type: "agent_restored"; agent: AgentSummary; by: HubActor }
  | { type: "agent_deleted"; name: string; by: HubActor }
  | { type: "agent_activity"; name: string; busy: boolean; unread: number }
  | { type: "notice"; level: HubNoticeLevel; message: string; agent?: string }
  | { type: "workspace_changed"; changes: WorkspaceChange[] }
  | Extract<ServerMessage, { type: "workspace_resync" | "workspace_watch_unavailable" }>;

/**
 * The one client-to-server frame on `/api/hub/ws`. A prefix is `team` or a
 * path under `team/`, the spelling the hub's change feed uses.
 */
export type HubClientMessage = { type: "watch_team"; prefixes: string[] };

/** Which file tree a workspace call addresses: an agent's, or the shared team's. */
export type WorkspaceScope = "agent" | "team";
