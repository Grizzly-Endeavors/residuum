// What the page says when the agent reloads its settings. The agent answers a
// reload request with a `reloading` frame, and then reports how the reload
// went in a `notice`; a reload that the user's own file edit set off reports
// too, and the settings watcher can follow a reload with a second one that
// finds nothing left to apply.
//
// A control that already says what happens (the composer's model control
// tells the user a change applies from the next reply) asks for a reload
// quietly, so the reload's success reports don't repeat it in the agent's
// words. A failed reload always speaks.

import type { NotificationKind } from "./notifications.svelte";

/**
 * How long after a quiet request the reload's success reports stay unsaid:
 * the reload itself, and the settings watcher's own pass over the same file,
 * which follows within a few seconds.
 */
const QUIET_WINDOW_MS = 15_000;

/** A success report from the agent: "configuration reloaded: models", "configuration reloaded: no changes detected". */
const RELOADED = /^(hub )?configuration reloaded\b/i;
const NOTHING_CHANGED = /no changes detected/i;
/** A reload the agent couldn't apply, which it keeps running without. */
const RELOAD_FAILED = /^(hub )?config(uration)? reload failed\b/i;

class QuietReloads {
  /** Quiet requests whose `reloading` frame hasn't come back yet. */
  private pending = 0;
  private until = 0;

  /** A control that already says what happens is asking for a reload. */
  expect(): void {
    if (Date.now() >= this.until) this.pending = 0;
    this.pending += 1;
    this.until = Date.now() + QUIET_WINDOW_MS;
  }

  /** Whether this `reloading` frame answers a quiet request, which it then settles. */
  takeReloading(): boolean {
    if (Date.now() >= this.until) this.pending = 0;
    if (this.pending === 0) return false;
    this.pending -= 1;
    return true;
  }

  /** Whether a reload's success report falls inside a quiet request's window. */
  get quiet(): boolean {
    return Date.now() < this.until;
  }
}

export const quietReloads = new QuietReloads();

/** What to surface, and how. */
export interface ReloadNotice {
  kind: NotificationKind;
  message: string;
  /** The agent's own words, behind the recall list's expandable detail. */
  details?: string;
}

/** The toast for the agent's `reloading` frame, or `null` when a quiet request asked for it. */
export function reloadingNotice(): ReloadNotice | null {
  if (quietReloads.takeReloading()) return null;
  return { kind: "system", message: "Reloading settings…" };
}

/**
 * The toast for a `notice` frame, or `null` for a reload's success report a
 * quiet request covers. Reload reports are put in plain words with the
 * agent's own behind the detail; every other notice is shown as it came.
 */
export function noticeFrameNotice(message: string): ReloadNotice | null {
  if (RELOAD_FAILED.test(message)) {
    return {
      kind: "error",
      message: "Residuum couldn't apply the new settings and is still using the old ones.",
      details: message,
    };
  }
  if (!RELOADED.test(message)) return { kind: "notice", message };
  if (quietReloads.quiet) return null;
  return {
    kind: "notice",
    message: NOTHING_CHANGED.test(message)
      ? "Settings reloaded. Nothing had changed."
      : "Settings reloaded.",
    details: message,
  };
}
