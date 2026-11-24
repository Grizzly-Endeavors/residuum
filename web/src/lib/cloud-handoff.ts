export const CLOUD_HANDOFF_PARAM = "cloud";

export type CloudHandoff = "connecting" | "missing-token" | "save-failed" | "not-started-here";

const OUTCOMES: ReadonlySet<string> = new Set<CloudHandoff>([
  "connecting",
  "missing-token",
  "save-failed",
  "not-started-here",
]);

function isCloudHandoff(value: string): value is CloudHandoff {
  return OUTCOMES.has(value);
}

export function handoffOf(search: string): CloudHandoff | null {
  const value = new URLSearchParams(search).get(CLOUD_HANDOFF_PARAM);
  return value !== null && isCloudHandoff(value) ? value : null;
}

let pending: CloudHandoff | null = null;

export function captureCloudHandoff(search: string): void {
  pending = handoffOf(search);
}

export function takeCloudHandoff(): CloudHandoff | null {
  const outcome = pending;
  pending = null;
  return outcome;
}

export function handoffProblem(outcome: CloudHandoff | null): string | null {
  switch (outcome) {
    case "missing-token":
      return "Residuum Cloud sent you back without signing this install in. Try connecting again.";
    case "save-failed":
      return "You signed in, but Residuum couldn't save the sign-in on this machine. Try connecting again; if it fails again, the details are in Residuum's logs.";
    case "not-started-here":
      return "This sign-in link didn't come from this app, or it expired, so Residuum didn't use it. Try connecting again.";
    case "connecting":
    case null:
      return null;
  }
}
