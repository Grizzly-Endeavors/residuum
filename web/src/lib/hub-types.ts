// ── Hub types ────────────────────────────────────────────────────────
//
// Shapes for the hub API and `/api/hub/ws`. The backend exports the agent
// shapes, the list envelopes and every hub WebSocket frame through ts-rs
// (`src/hub/types.rs`); they are re-exported from `generated/`. Only the
// status and stop-all bodies, which the backend builds inline, are
// hand-written here, and `HubServerMessage` joins the generated frame types.

import type { ServerMessage } from "./generated/ServerMessage";
import type { AgentSummary } from "./generated/AgentSummary";
import type { HubEvent } from "./generated/HubEvent";
import type { HubSocketFrame } from "./generated/HubSocketFrame";

export type { A2aVisibility } from "./generated/A2aVisibility";
export type { Actor } from "./generated/Actor";
export type { AgentActivity } from "./generated/AgentActivity";
export type { AgentErrorKind } from "./generated/AgentErrorKind";
export type { AgentLastError } from "./generated/AgentLastError";
export type { AgentListResponse } from "./generated/AgentListResponse";
export type { AgentPatch } from "./generated/AgentPatch";
export type { AgentState } from "./generated/AgentState";
export type { AgentSummary } from "./generated/AgentSummary";
export type { CreateAgentRequest } from "./generated/CreateAgentRequest";
export type { DeleteOutcome } from "./generated/DeleteOutcome";
export type { DeletedAgent } from "./generated/DeletedAgent";
export type { DeletedAgentListResponse } from "./generated/DeletedAgentListResponse";
export type { HubClientMessage } from "./generated/HubClientMessage";
export type { HubEvent } from "./generated/HubEvent";
export type { HubSocketFrame } from "./generated/HubSocketFrame";
export type { NoticeLevel } from "./generated/NoticeLevel";
export type { RestoreAgentRequest } from "./generated/RestoreAgentRequest";
export type { TeamEvent } from "./generated/TeamEvent";
export type { TeamEventKind } from "./generated/TeamEventKind";
export type { TeamEventLevel } from "./generated/TeamEventLevel";
export type { TeamEventPage } from "./generated/TeamEventPage";
export type { TeamEventPlace } from "./generated/TeamEventPlace";
export type { TeamEventTarget } from "./generated/TeamEventTarget";

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

/** The team change frames, which the hub sends with the agent protocol's shapes. */
export type HubWorkspaceFrame = Extract<
  ServerMessage,
  { type: "workspace_changed" | "workspace_resync" | "workspace_watch_unavailable" }
>;

/** Server-to-client frames on `/api/hub/ws`. */
export type HubServerMessage = HubSocketFrame | HubEvent | HubWorkspaceFrame;

/** Which file tree a workspace call addresses: the current agent's, or the shared team's. */
export type WorkspaceScope = "agent" | "team";
