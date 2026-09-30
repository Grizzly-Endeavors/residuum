import { describe, expectTypeOf, it } from "vitest";
import type { AgentSummary as GeneratedAgentSummary } from "./generated/AgentSummary";
import type { AgentState as GeneratedAgentState } from "./generated/AgentState";
import type { A2aVisibility as GeneratedA2aVisibility } from "./generated/A2aVisibility";
import type { AgentActivity as GeneratedAgentActivity } from "./generated/AgentActivity";
import type { AgentErrorKind as GeneratedAgentErrorKind } from "./generated/AgentErrorKind";
import type { AgentListResponse as GeneratedAgentListResponse } from "./generated/AgentListResponse";
import type { HubClientMessage as GeneratedHubClientMessage } from "./generated/HubClientMessage";
import type { AgentPatch as GeneratedAgentPatch } from "./generated/AgentPatch";
import type { CreateAgentRequest as GeneratedCreateAgentRequest } from "./generated/CreateAgentRequest";
import type { DeleteOutcome as GeneratedDeleteOutcome } from "./generated/DeleteOutcome";
import type {
  A2aVisibility,
  AgentActivity,
  AgentErrorKind,
  AgentLastError,
  AgentListResponse,
  AgentPatch,
  AgentState,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
  HubClientMessage,
  HubServerMessage,
} from "./hub-types";

// The agent shapes are the backend's ts-rs exports, not copies of them: these
// fail to compile if `hub-types` ever declares its own again.
describe("hub types", () => {
  it("re-exports the shapes the backend generates", () => {
    expectTypeOf<AgentSummary>().toEqualTypeOf<GeneratedAgentSummary>();
    expectTypeOf<AgentState>().toEqualTypeOf<GeneratedAgentState>();
    expectTypeOf<A2aVisibility>().toEqualTypeOf<GeneratedA2aVisibility>();
    expectTypeOf<AgentActivity>().toEqualTypeOf<GeneratedAgentActivity>();
    expectTypeOf<AgentPatch>().toEqualTypeOf<GeneratedAgentPatch>();
    expectTypeOf<CreateAgentRequest>().toEqualTypeOf<GeneratedCreateAgentRequest>();
    expectTypeOf<DeleteOutcome>().toEqualTypeOf<GeneratedDeleteOutcome>();
  });

  it("carries the generated summary in the hub frames", () => {
    type StateFrame = Extract<HubServerMessage, { type: "agent_state" }>;
    expectTypeOf<StateFrame["agent"]>().toEqualTypeOf<GeneratedAgentSummary>();
  });

  it("re-exports the list envelope and the client message the backend generates", () => {
    expectTypeOf<AgentListResponse>().toEqualTypeOf<GeneratedAgentListResponse>();
    expectTypeOf<HubClientMessage>().toEqualTypeOf<GeneratedHubClientMessage>();
    expectTypeOf<AgentErrorKind>().toEqualTypeOf<GeneratedAgentErrorKind>();
  });

  it("covers every frame the hub sends", () => {
    expectTypeOf<HubServerMessage["type"]>().toEqualTypeOf<
      | "hub_boot"
      | "agents_snapshot"
      | "agent_state"
      | "agent_stopping"
      | "agent_created"
      | "agent_restored"
      | "agent_deleted"
      | "agent_activity"
      | "notice"
      | "hub_config_reloaded"
      | "workspace_changed"
      | "workspace_resync"
      | "workspace_watch_unavailable"
    >();
  });

  it("gives a snapshot the activity and stopping set beside the agents", () => {
    type Snapshot = Extract<HubServerMessage, { type: "agents_snapshot" }>;
    expectTypeOf<Snapshot["activity"]>().toEqualTypeOf<Record<string, AgentActivity>>();
    expectTypeOf<Snapshot["stopping"]>().toEqualTypeOf<string[]>();
    expectTypeOf<AgentActivity["busy_since"]>().toEqualTypeOf<string | null>();
  });

  it("describes a failure by kind and underlying reason", () => {
    expectTypeOf<AgentErrorKind>().toEqualTypeOf<"config" | "port_conflict" | "crash" | "other">();
    expectTypeOf<AgentLastError["reason"]>().toEqualTypeOf<string>();
  });

  it("includes every state the backend can report, starting among them", () => {
    expectTypeOf<AgentState>().toEqualTypeOf<"starting" | "running" | "stopped" | "failed">();
  });
});
