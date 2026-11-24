import type { AutoModeVerdict } from "../lib/generated/AutoModeVerdict";

function percent(probability: number | null): string {
  return probability === null ? "" : ` (${Math.round(probability * 100)}% sure)`;
}

export function blockedByAutoMode(verdict: AutoModeVerdict | undefined): boolean {
  return verdict?.decision === "blocked";
}

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
