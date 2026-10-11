// The words for Auto Mode's verdict on a tool call: the row's status when it
// blocked the call, and the note in the step's details for every checked call.

import type { AutoModeVerdict } from "../lib/generated/AutoModeVerdict";

function percent(probability: number | null): string {
  return probability === null ? "" : ` (${Math.round(probability * 100)}% sure)`;
}

/** Whether Auto Mode stopped this call from running. */
export function blockedByAutoMode(verdict: AutoModeVerdict | undefined): boolean {
  return verdict?.decision === "blocked";
}

/** The sentence a step's details show about Auto Mode's check. */
export function autoModeNote(verdict: AutoModeVerdict): string {
  switch (verdict.decision) {
    case "blocked":
      return `Blocked by Auto Mode: it matches the rule “${verdict.rule ?? "a deny rule"}”${percent(verdict.probability)}. It didn't run.`;
    case "allowed":
      return verdict.rule === null
        ? "Checked by Auto Mode: no rule matched."
        : `Allowed by Auto Mode's exception “${verdict.rule}”${percent(verdict.probability)}.`;
    case "unchecked":
      return `Not checked by Auto Mode: ${verdict.reason ?? "the decision model didn't answer."}`;
  }
}
