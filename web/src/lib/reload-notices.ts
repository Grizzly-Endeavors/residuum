import type { NotificationKind } from "./notifications.svelte";
import type { ReloadOutcome } from "./generated/ReloadOutcome";
import type { ReloadedFiles } from "./generated/ReloadedFiles";

const QUIET_WINDOW_MS = 15_000;

class QuietReloads {
  private pending = 0;
  private until = 0;

  expect(): void {
    if (Date.now() >= this.until) this.pending = 0;
    this.pending += 1;
    this.until = Date.now() + QUIET_WINDOW_MS;
  }

  takeReloading(): boolean {
    if (Date.now() >= this.until) this.pending = 0;
    if (this.pending === 0) return false;
    this.pending -= 1;
    return true;
  }

  get quiet(): boolean {
    return Date.now() < this.until;
  }
}

export const quietReloads = new QuietReloads();

export interface ReloadNotice {
  kind: NotificationKind;
  message: string;
  details?: string;
}

export function reloadingNotice(): ReloadNotice | null {
  if (quietReloads.takeReloading()) return null;
  return { kind: "system", message: "Reloading settings…" };
}

export interface NoticeFrame {
  message: string;
  reload?: ReloadOutcome;
  reload_of?: ReloadedFiles;
}

const FAILED_WORDS: Record<ReloadedFiles, string> = {
  agent_config: "Residuum couldn't apply the new settings and is still using the old ones.",
  hub_config:
    "This agent couldn't apply the new settings for all agents and is still using the old ones.",
  workspace:
    "Some of the agent's files couldn't be reloaded. Those parts keep their previous settings.",
};

export function noticeFrameNotice(frame: NoticeFrame): ReloadNotice | null {
  const { message, reload, reload_of: files } = frame;
  if (reload === undefined || files === undefined) return { kind: "notice", message };
  if (reload === "failed") return { kind: "error", message: FAILED_WORDS[files], details: message };
  switch (files) {
    case "hub_config":
      return null;
    case "workspace":
      return { kind: "notice", message: "Agent files reloaded.", details: message };
    case "agent_config":
      if (quietReloads.quiet) return null;
      return {
        kind: "notice",
        message:
          reload === "unchanged" ? "Settings reloaded. Nothing had changed." : "Settings reloaded.",
        details: message,
      };
  }
}

export interface HubConfigReloadedFrame {
  ok: boolean;
  changed: boolean;
  message: string | null;
}

export function hubReloadOutcome(frame: HubConfigReloadedFrame): ReloadOutcome {
  if (!frame.ok) return "failed";
  return frame.changed ? "reloaded" : "unchanged";
}

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
