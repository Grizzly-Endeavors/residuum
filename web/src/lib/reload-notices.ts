// What the page says when settings are reloaded. Every reload report comes
// with a structured outcome, and the page words its toast from that, keeping
// the server's own account behind the expandable detail:
//
// - The agent's socket answers a reload request with a `reloading` frame, and
//   reports how each reload went in a `notice` whose `reload` field is
//   `reloaded`, `unchanged` or `failed` and whose `reload_of` names the files:
//   the agent's own config, the hub's, or the agent's workspace files. A
//   reload the user's own file edit set off reports too, and the settings
//   watcher can follow a reload with a second one that finds nothing left to
//   apply.
// - The hub's socket reports each reload of the settings for all agents in a
//   `hub_config_reloaded` frame. That is the one report of a hub reload: each
//   agent then reloads the hub's config too, and its socket's report of that
//   is shown only when the agent couldn't apply it.
//
// A control that already says what happens (the composer's model control
// tells the user a change applies from the next reply) asks for a reload
// quietly, so the reload's success reports don't repeat it. A failed reload
// always speaks.

import type { NotificationKind } from "./notifications.svelte";
import type { ReloadOutcome } from "./generated/ReloadOutcome";
import type { ReloadedFiles } from "./generated/ReloadedFiles";

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
  reload_of?: ReloadedFiles;
}

/** The plain words for each outcome of a reload of one set of files. */
const RELOAD_WORDS: Record<ReloadedFiles, Record<ReloadOutcome, string>> = {
  agent_config: {
    reloaded: "Settings reloaded.",
    unchanged: "Settings reloaded. Nothing had changed.",
    failed: "Residuum couldn't apply the new settings and is still using the old ones.",
  },
  hub_config: {
    reloaded: "This agent took up the new settings for all agents.",
    unchanged: "This agent's settings for all agents hadn't changed.",
    failed:
      "This agent couldn't apply the new settings for all agents and is still using the old ones.",
  },
  workspace: {
    reloaded: "Agent files reloaded.",
    unchanged: "Agent files reloaded. Nothing had changed.",
    failed: "Residuum couldn't reload the agent's files and is still using the old ones.",
  },
};

/**
 * The toast for an agent socket `notice` frame, or `null` for a success
 * report nobody needs: one a quiet request covers, or the agent's own reload
 * of the hub's config, which the hub socket already reported. A reload report
 * is put in plain words with the agent's own behind the detail; a failed one
 * always shows; any other notice is shown as it came.
 */
export function noticeFrameNotice(frame: NoticeFrame): ReloadNotice | null {
  const { message, reload } = frame;
  if (reload === undefined) return { kind: "notice", message };
  const files = frame.reload_of ?? "agent_config";
  const words = RELOAD_WORDS[files][reload];
  if (reload === "failed") return { kind: "error", message: words, details: message };
  if (files === "hub_config" || quietReloads.quiet) return null;
  return { kind: "notice", message: words, details: message };
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
