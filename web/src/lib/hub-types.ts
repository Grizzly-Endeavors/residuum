// ── Hub types (hand-written from the HTTP contract) ──────────────────
//
// Shapes for the hub API and `/api/hub/ws`, written against
// docs/design/multi-agent-hub/http-contract.md. The backend does not yet
// generate them; when it does, replace these with the generated exports.

import type { RepoKind, WorkspaceChange } from "./types";

export type AgentState = "starting" | "running" | "stopped" | "failed";

export type A2aVisibility = "public" | "private";

export interface AgentLastError {
  message: string;
  /** RFC 3339 timestamp. */
  at: string;
}

export interface AgentSummary {
  name: string;
  state: AgentState;
  /** Set only while `state` is `failed`. */
  last_error: AgentLastError | null;
  autostart: boolean;
  /** One-line role from the agent's wiki role page. */
  role: string | null;
  a2a_visibility: A2aVisibility;
}

export interface AgentListResponse {
  agents: AgentSummary[];
}

/** Who caused a hub event: the user (UI or CLI), or a teammate agent. */
export type HubActor = "user" | `agent:${string}`;

/** `POST /api/hub/agents` body. Without `models_from`, `providers_toml` is required. */
export interface CreateAgentRequest {
  name: string;
  description?: string;
  /** An existing agent whose `providers.toml` is copied. */
  models_from?: string;
  /** Raw `providers.toml`, when `models_from` is not given. */
  providers_toml?: string;
  a2a_visibility?: A2aVisibility;
}

export interface DeleteAgentResponse {
  deleted: boolean;
  checkpoint_id: string | null;
}

export interface HubStatusResponse {
  version: string;
  uptime_secs: number;
  /** Same shape as the cloud status response. */
  tunnel: unknown;
  agents: { running: number; stopped: number; failed: number };
}

export type HubNoticeLevel = "info" | "warn" | "error";

/** Server-to-client frames on `/api/hub/ws`. */
export type HubServerMessage =
  | { type: "agents_snapshot"; agents: AgentSummary[] }
  | { type: "agent_state"; agent: AgentSummary }
  | { type: "agent_created"; agent: AgentSummary; by: HubActor }
  | { type: "agent_deleted"; name: string; by: HubActor }
  | { type: "agent_activity"; name: string; busy: boolean; unread: number }
  | { type: "notice"; level: HubNoticeLevel; message: string; agent?: string }
  | { type: "workspace_changed"; changes: WorkspaceChange[] };

/** The one client-to-server frame on `/api/hub/ws`. */
export type HubClientMessage = { type: "watch_team"; prefixes: string[] };

/**
 * Checkpoint repositories. `workspace` and `agent_config` belong to an agent;
 * `hub` and `team` are hub-level.
 */
export type CheckpointRepo = RepoKind | "team";

/** Which file tree a workspace call addresses: the current agent's, or the shared team's. */
export type WorkspaceScope = "agent" | "team";
