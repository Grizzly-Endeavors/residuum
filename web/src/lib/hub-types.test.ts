import { describe, expectTypeOf, it } from "vitest";
import type { AgentSummary as GeneratedAgentSummary } from "./generated/AgentSummary";
import type { AgentState as GeneratedAgentState } from "./generated/AgentState";
import type { A2aVisibility as GeneratedA2aVisibility } from "./generated/A2aVisibility";
import type { AgentActivity as GeneratedAgentActivity } from "./generated/AgentActivity";
import type { AgentPatch as GeneratedAgentPatch } from "./generated/AgentPatch";
import type { CreateAgentRequest as GeneratedCreateAgentRequest } from "./generated/CreateAgentRequest";
import type { DeleteOutcome as GeneratedDeleteOutcome } from "./generated/DeleteOutcome";
import type {
  A2aVisibility,
  AgentActivity,
  AgentPatch,
  AgentState,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
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

  it("includes every state the backend can report, starting among them", () => {
    expectTypeOf<AgentState>().toEqualTypeOf<"starting" | "running" | "stopped" | "failed">();
  });
});
