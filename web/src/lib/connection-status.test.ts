import { describe, expect, it } from "vitest";
import {
  connectionStatusRows,
  connectionStatusText,
  type ConnectionFacts,
} from "./connection-status";

function facts(overrides: Partial<ConnectionFacts> = {}): ConnectionFacts {
  return {
    agent: "atlas",
    state: "running",
    hubConnection: "connected",
    agentConnection: "connected",
    ...overrides,
  };
}

describe("connectionStatusRows", () => {
  it("names Residuum, the agent and its connection", () => {
    expect(connectionStatusRows(facts())).toEqual([
      { label: "Residuum", value: "Connected" },
      { label: "Agent", value: "atlas" },
      { label: "Connection", value: "Connected" },
    ]);
  });

  it("says when the agent is reconnecting, and when it isn't running", () => {
    expect(connectionStatusRows(facts({ agentConnection: "connecting" }))[2]).toEqual({
      label: "Connection",
      value: "Reconnecting. Messages you send will go out once it's back.",
    });
    expect(connectionStatusRows(facts({ state: "stopped" }))[2]).toEqual({
      label: "Connection",
      value: "Not running, so there's no connection to it.",
    });
  });

  it("says when Residuum itself can't be reached, and when no agent is open", () => {
    expect(connectionStatusRows(facts({ hubConnection: "disconnected", agent: null }))).toEqual([
      { label: "Residuum", value: "Can't reach Residuum right now. Trying again." },
      { label: "Agent", value: "None open" },
    ]);
  });

  it("copies as one line per row", () => {
    expect(connectionStatusText(connectionStatusRows(facts()))).toBe(
      "Residuum: Connected\nAgent: atlas\nConnection: Connected",
    );
  });
});
