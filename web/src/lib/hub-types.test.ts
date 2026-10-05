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
import type { ServerMessage } from "./generated/ServerMessage";
import type {
  A2aVisibility,
  AgentActivity,
  AgentErrorKind,
  AgentLastError,
  AgentListResponse,
  AgentOverview,
  AgentPatch,
  AgentState,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
  HubClientMessage,
  HubServerMessage,
  LastMessage,
  LiveSession,
  OverviewResponse,
  SessionSubscriptionKind,
  TeamEvent,
  TeamEventKind,
  TeamEventLevel,
  TeamEventPage,
  TeamEventTarget,
  TimePrecision,
  UpcomingKind,
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
      | "team_event"
      | "agent_overview"
      | "artifact_updated"
      | "artifact_removed"
      | "subscribed"
      | "session_frame"
      | "session_relay_lagged"
      | "system_one_status"
      | "workspace_changed"
      | "workspace_resync"
      | "workspace_watch_unavailable"
    >();
  });

  it("names the agent and carries the agent socket's frame in a session frame", () => {
    type Frame = Extract<HubServerMessage, { type: "session_frame" }>;
    expectTypeOf<Frame["agent"]>().toEqualTypeOf<string>();
    expectTypeOf<Frame["frame"]>().toEqualTypeOf<ServerMessage>();
  });

  it("acknowledges a subscription with its kind and the fields that name it", () => {
    type Ack = Extract<HubServerMessage, { type: "subscribed" }>;
    expectTypeOf<Ack["kind"]>().toEqualTypeOf<SessionSubscriptionKind>();
    expectTypeOf<SessionSubscriptionKind>().toEqualTypeOf<"session" | "artifact_sessions">();
    expectTypeOf<Ack["agent"]>().toEqualTypeOf<string | undefined>();
    expectTypeOf<Ack["address"]>().toEqualTypeOf<string | undefined>();
    expectTypeOf<Ack["artifact"]>().toEqualTypeOf<string | undefined>();
  });

  it("sends the session subscriptions as client messages", () => {
    expectTypeOf<HubClientMessage["type"]>().toEqualTypeOf<
      | "watch_team"
      | "presence"
      | "subscribe_session"
      | "unsubscribe_session"
      | "subscribe_artifact_sessions"
      | "unsubscribe_artifact_sessions"
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

  it("describes team events by kind, level and the place they point to", () => {
    expectTypeOf<TeamEventKind>().toEqualTypeOf<
      | "hub_started"
      | "agent_started"
      | "agent_stopped"
      | "agent_failed"
      | "agent_created"
      | "agent_deleted"
      | "agent_restored"
      | "agent_replied"
      | "session_started"
      | "session_finished"
      | "inbox_item_added"
      | "scheduled_run_finished"
      | "hub_notice"
    >();
    expectTypeOf<TeamEventLevel>().toEqualTypeOf<"info" | "warn" | "error">();
    expectTypeOf<TeamEventTarget["kind"]>().toEqualTypeOf<
      "agent_place" | "session" | "inbox_item"
    >();
    expectTypeOf<TeamEvent["id"]>().toEqualTypeOf<number>();
    expectTypeOf<TeamEventPage["next_before"]>().toEqualTypeOf<number | null>();
  });

  it("carries the boot id and the event in the team event frame", () => {
    type Frame = Extract<HubServerMessage, { type: "team_event" }>;
    expectTypeOf<Frame["boot_id"]>().toEqualTypeOf<string>();
    expectTypeOf<Frame["event"]>().toEqualTypeOf<TeamEvent>();
  });

  it("describes an agent's overview by its last message, live sessions and inbox count", () => {
    expectTypeOf<OverviewResponse["agents"]>().toEqualTypeOf<AgentOverview[]>();
    expectTypeOf<AgentOverview["last_message"]>().toEqualTypeOf<LastMessage | null>();
    expectTypeOf<AgentOverview["inbox_unread"]>().toEqualTypeOf<number>();
    expectTypeOf<LastMessage["role"]>().toEqualTypeOf<"user" | "assistant">();
    expectTypeOf<TimePrecision>().toEqualTypeOf<"minute" | "day">();
    expectTypeOf<UpcomingKind>().toEqualTypeOf<"pulse" | "action">();
    expectTypeOf<LiveSession["source_label"]>().toEqualTypeOf<string>();
  });

  it("carries the whole overview in the agent overview frame", () => {
    type Frame = Extract<HubServerMessage, { type: "agent_overview" }>;
    expectTypeOf<Frame["overview"]>().toEqualTypeOf<AgentOverview>();
  });

  it("includes every state the backend can report, starting among them", () => {
    expectTypeOf<AgentState>().toEqualTypeOf<"starting" | "running" | "stopped" | "failed">();
  });
});
