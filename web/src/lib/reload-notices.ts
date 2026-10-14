// What the page says when settings are reloaded. Every reload report comes
// with a structured outcome, and the page words its toast from that, keeping
// the server's own account behind the expandable detail:
//
// - The agent's socket answers a reload request with a `reloading` frame, and
//   reports how each reload of the agent's settings went in a `notice` whose
//   `reload` field is `reloaded`, `unchanged` or `failed`. A reload the user's
//   own file edit set off reports too, and the settings watcher can follow a
//   reload with a second one that finds nothing left to apply.
// - The hub's socket reports each reload of the settings for all agents in a
//   `hub_config_reloaded` frame.
//
// A control that already says what happens (the composer's model control
// tells the user a change applies from the next reply) asks for a reload
// quietly, so the reload's success reports don't repeat it. A failed reload
// always speaks.

import type { NotificationKind } from "./notifications.svelte";
import type { ReloadOutcome } from "./generated/ReloadOutcome";

/**
 * How long after a quiet request the reload's success reports stay unsaid:
 * the reload itself, and the settings watcher's own pass over the same file,
 * which follows within a few seconds.
 */
const QUIET_WINDOW_MS = 15_000;

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
  /** The server's own words, behind the recall list's expandable detail. */
  details?: string;
}

/** The toast for the agent's `reloading` frame, or `null` when a quiet request asked for it. */
export function reloadingNotice(): ReloadNotice | null {
  if (quietReloads.takeReloading()) return null;
  return { kind: "system", message: "Reloading settings…" };
}

/** The agent socket's `notice` frame, as far as these toasts read it. */
export interface NoticeFrame {
  message: string;
  reload?: ReloadOutcome;
}

/**
 * The toast for an agent socket `notice` frame, or `null` for a reload's
 * success report a quiet request covers. A reload report is put in plain
 * words with the agent's own behind the detail; any other notice is shown as
 * it came.
 */
export function noticeFrameNotice(frame: NoticeFrame): ReloadNotice | null {
  const { message, reload } = frame;
  switch (reload) {
    case undefined:
      return { kind: "notice", message };
    case "failed":
      return {
        kind: "error",
        message: "Residuum couldn't apply the new settings and is still using the old ones.",
        details: message,
      };
    case "reloaded":
    case "unchanged":
      if (quietReloads.quiet) return null;
      return {
        kind: "notice",
        message:
          reload === "unchanged" ? "Settings reloaded. Nothing had changed." : "Settings reloaded.",
        details: message,
      };
  }
}

/** The hub socket's `hub_config_reloaded` frame. */
export interface HubConfigReloadedFrame {
  ok: boolean;
  changed: boolean;
  message: string | null;
}

/** What a `hub_config_reloaded` frame reports. */
export function hubReloadOutcome(frame: HubConfigReloadedFrame): ReloadOutcome {
  if (!frame.ok) return "failed";
  return frame.changed ? "reloaded" : "unchanged";
}

/**
 * The toast for the hub's report of a reload of the settings for all agents,
 * or `null` when the reload found nothing new: the hub reloads whenever its
 * file is saved, and nobody asked to hear that nothing changed.
 */
export function hubReloadNotice(frame: HubConfigReloadedFrame): ReloadNotice | null {
  const details = frame.message ?? undefined;
  switch (hubReloadOutcome(frame)) {
    case "failed":
      return {
        kind: "error",
        message:
          "Residuum couldn't apply the new settings for all agents and is still using the old ones.",
        details,
      };
    case "reloaded":
      return { kind: "notice", message: "Settings for all agents reloaded.", details };
    case "unchanged":
      return null;
  }
}
