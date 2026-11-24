import type { AgentListResponse, AgentSummary, HubServerMessage } from "../lib/hub-types";

export function snapshot(
  agents: AgentSummary[],
  rest: Partial<Pick<AgentListResponse, "activity" | "stopping">> = {},
): HubServerMessage {
  return { type: "agents_snapshot", agents, activity: {}, stopping: [], ...rest };
}

export function activityFrame(
  name: string,
  busy: boolean,
  unread: number,
  busySince: string | null = null,
): HubServerMessage {
  return { type: "agent_activity", name, busy, busy_since: busySince, unread };
}
