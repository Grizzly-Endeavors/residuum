// The facts "Show connection status" puts in its dialog: whether Residuum is
// reachable, which agent is bound, and whether that agent's connection is up.

import type { AgentDisplayState } from "./agent-display-state";
import type { ConnectionStatus } from "./types";

export interface ConnectionFacts {
  agent: string | null;
  state: AgentDisplayState | null;
  hubConnection: ConnectionStatus;
  agentConnection: ConnectionStatus;
}

export interface StatusRow {
  label: string;
  value: string;
}

/** Residuum, the agent, and its connection. The model is a separate row. */
export function connectionStatusRows(facts: ConnectionFacts): StatusRow[] {
  const rows: StatusRow[] = [
    {
      label: "Residuum",
      value:
        facts.hubConnection === "connected"
          ? "Connected"
          : "Can't reach Residuum right now. Trying again.",
    },
  ];
  if (facts.agent === null) {
    rows.push({ label: "Agent", value: "None open" });
    return rows;
  }
  rows.push({ label: "Agent", value: facts.agent });
  let connection: string;
  if (facts.state !== "running" && facts.state !== "stopping") {
    connection = "Not running, so there's no connection to it.";
  } else if (facts.agentConnection === "connected") {
    connection = "Connected";
  } else {
    connection = "Reconnecting. Messages you send will go out once it's back.";
  }
  rows.push({ label: "Connection", value: connection });
  return rows;
}

/** The rows as plain text, one per line, for the clipboard. */
export function connectionStatusText(rows: readonly StatusRow[]): string {
  return rows.map((row) => `${row.label}: ${row.value}`).join("\n");
}
