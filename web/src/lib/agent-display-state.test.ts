import { beforeEach, describe, expect, it } from "vitest";
import { snapshot } from "../test/hub-frames";
import { displayState } from "./agent-display-state";
import { HubStore } from "./hub.svelte";
import type { AgentState, AgentSummary } from "./hub-types";

function agent(name: string, state: AgentState): AgentSummary {
  return {
    name,
    display_name: name,
    state,
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

describe("displayState", () => {
  it("shows the hub's state for an agent nobody is stopping", () => {
    for (const state of ["starting", "running", "stopped", "failed"] as const) {
      expect(displayState(state, false)).toBe(state);
    }
  });

  it("shows a running or starting agent in the stopping set as stopping", () => {
    expect(displayState("running", true)).toBe("stopping");
    expect(displayState("starting", true)).toBe("stopping");
  });

  it("shows an agent that has already stopped or failed as it is, whatever the set says", () => {
    expect(displayState("stopped", true)).toBe("stopped");
    expect(displayState("failed", true)).toBe("failed");
  });
});

describe("HubStore.displayStateOf", () => {
  let store: HubStore;

  beforeEach(() => {
    store = new HubStore();
  });

  it("knows nothing of an agent the list doesn't name", () => {
    expect(store.displayStateOf("atlas")).toBeNull();
  });

  it("reads stopping from the snapshot's stopping set", () => {
    store.handleFrame(snapshot([agent("atlas", "running")], { stopping: ["atlas"] }));
    expect(store.displayStateOf("atlas")).toBe("stopping");
  });

  it("is stopping from agent_stopping until the next agent_state", () => {
    store.handleFrame(snapshot([agent("atlas", "running")]));
    expect(store.displayStateOf("atlas")).toBe("running");

    store.handleFrame({ type: "agent_stopping", name: "atlas" });
    expect(store.displayStateOf("atlas")).toBe("stopping");

    store.handleFrame({ type: "agent_state", agent: agent("atlas", "stopped") });
    expect(store.displayStateOf("atlas")).toBe("stopped");
  });

  it("follows a start that fails", () => {
    store.handleFrame(snapshot([agent("brittle", "stopped")]));
    store.handleFrame({ type: "agent_state", agent: agent("brittle", "starting") });
    expect(store.displayStateOf("brittle")).toBe("starting");
    store.handleFrame({ type: "agent_state", agent: agent("brittle", "failed") });
    expect(store.displayStateOf("brittle")).toBe("failed");
  });
});
