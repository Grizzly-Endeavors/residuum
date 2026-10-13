// ── The Residuum Cloud sign-in hand-off ──────────────────────────────
//
// After someone signs in on the relay, the relay sends that tab to the
// gateway's `/cloud/callback`, which saves the token and redirects into the
// app at Settings → Residuum Cloud with `cloud=<outcome>`
// (`CallbackOutcome` in `src/gateway/web/cloud.rs`). The router rewrites the
// address to its own form as it starts, which drops the parameter, so the
// outcome is read from the address before the app mounts and handed to the
// section once.

/** The query parameter the callback names its outcome in. */
export const CLOUD_HANDOFF_PARAM = "cloud";

/**
 * What the callback reports: the token is saved and the tunnel is starting,
 * the relay sent no token, or Residuum couldn't save it.
 */
export type CloudHandoff = "connecting" | "missing-token" | "save-failed";

const OUTCOMES: ReadonlySet<string> = new Set<CloudHandoff>([
  "connecting",
  "missing-token",
  "save-failed",
]);

function isCloudHandoff(value: string): value is CloudHandoff {
  return OUTCOMES.has(value);
}

/** The outcome a page's query names, or null when it names none this app knows. */
export function handoffOf(search: string): CloudHandoff | null {
  const value = new URLSearchParams(search).get(CLOUD_HANDOFF_PARAM);
  return value !== null && isCloudHandoff(value) ? value : null;
}

let pending: CloudHandoff | null = null;

/** Keep the outcome the page was opened with, before the router rewrites the address. */
export function captureCloudHandoff(search: string): void {
  pending = handoffOf(search);
}

/** The outcome the page was opened with, once: a later call, or a section opened again, gets null. */
export function takeCloudHandoff(): CloudHandoff | null {
  const outcome = pending;
  pending = null;
  return outcome;
}

/** What to tell the person about a hand-off that failed, or null when it didn't. */
export function handoffProblem(outcome: CloudHandoff | null): string | null {
  switch (outcome) {
    case "missing-token":
      return "Residuum Cloud sent you back without signing this install in. Try connecting again.";
    case "save-failed":
      return "You signed in, but Residuum couldn't save the sign-in on this machine. Try connecting again; if it fails again, the details are in Residuum's logs.";
    case "connecting":
    case null:
      return null;
  }
}
