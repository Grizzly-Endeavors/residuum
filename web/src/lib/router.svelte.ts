// The URL is the source of truth for where the user is. Components read the
// location from here and navigate through here; the layout derives its state
// from it rather than mounting a component per route, so every transition
// plays the same whether it came from a click or the browser's back button.
//
// History holds places, not panel states: opening a session or settings
// pushes an entry, toggling the workspace replaces the current one, so back
// moves between places the user visited.

import {
  formatLocation,
  MAIN_CHAT,
  parseLocation,
  type AppLocation,
  type ChatLocation,
  type WorkbenchLocation,
} from "./routes";
import type { SettingsSection } from "./types";

type HistoryMode = "push" | "replace";

class Router {
  chat = $state<ChatLocation>(MAIN_CHAT);
  settings = $state<SettingsSection | null>(null);
  workbench = $state<WorkbenchLocation | null>(null);
  scheduled = $state<boolean>(false);

  private started = false;

  /** Read the current URL and follow back/forward from then on. */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.syncFromUrl();
    window.addEventListener("popstate", () => this.syncFromUrl());
  }

  /** Show a run in the main pane, leaving settings or the workbench if open. */
  openSession(runId: string): void {
    this.go(
      { chat: { ...this.chat, runId }, settings: null, workbench: null, scheduled: false },
      "push",
    );
  }

  /** Return the main pane to the main chat, leaving settings or the workbench if open. */
  openMainChat(): void {
    this.go(
      { chat: { ...this.chat, runId: null }, settings: null, workbench: null, scheduled: false },
      "push",
    );
  }

  /**
   * Point the current place at another run without adding history, for when
   * the shown session continues in a new run.
   */
  replaceSession(runId: string): void {
    this.go(
      {
        chat: { ...this.chat, runId },
        settings: this.settings,
        workbench: this.workbench,
        scheduled: this.scheduled,
      },
      "replace",
    );
  }

  /**
   * Open or close the workspace panel. From settings or the workbench this is
   * a move back to the chat side, so it adds history like any other change of
   * place.
   */
  setWorkspace(open: boolean): void {
    const onChatSide = this.settings === null && this.workbench === null && !this.scheduled;
    this.go(
      {
        chat: { ...this.chat, workspace: open },
        settings: null,
        workbench: null,
        scheduled: false,
      },
      onChatSide ? "replace" : "push",
    );
  }

  openSettings(section: SettingsSection = "runtime"): void {
    this.go({ chat: this.chat, settings: section, workbench: null, scheduled: false }, "push");
  }

  /** Leave settings for the chat side as it was before settings opened. */
  closeSettings(): void {
    if (this.settings === null) return;
    this.go({ chat: this.chat, settings: null, workbench: null, scheduled: false }, "push");
  }

  /** Open the workbench: an artifact, or the artifact list when `artifact` is null. */
  openWorkbench(artifact: string | null = null): void {
    this.go(
      {
        chat: this.chat,
        settings: null,
        workbench: { artifact, full: false },
        scheduled: false,
      },
      "push",
    );
  }

  /**
   * Show the open artifact filling the window, or return it to the Residuum UI.
   * A view mode of the same place, so it replaces history rather than adding.
   */
  setWorkbenchFull(full: boolean): void {
    const artifact = this.workbench?.artifact ?? null;
    if (artifact === null) return;
    this.go(
      { chat: this.chat, settings: null, workbench: { artifact, full }, scheduled: false },
      "replace",
    );
  }

  /** Leave the workbench for the chat side as it was before it opened. */
  closeWorkbench(): void {
    if (this.workbench === null) return;
    this.go({ chat: this.chat, settings: null, workbench: null, scheduled: false }, "push");
  }

  /** Open the Scheduled view (pulses and scheduled actions). */
  openScheduled(): void {
    this.go({ chat: this.chat, settings: null, workbench: null, scheduled: true }, "push");
  }

  /** Leave the Scheduled view for the chat side as it was before it opened. */
  closeScheduled(): void {
    if (!this.scheduled) return;
    this.go({ chat: this.chat, settings: null, workbench: null, scheduled: false }, "push");
  }

  private go(location: AppLocation, mode: HistoryMode): void {
    const url = formatLocation(location);
    const current = `${window.location.pathname}${window.location.search}`;
    this.chat = location.chat;
    this.settings = location.settings;
    this.workbench = location.workbench;
    this.scheduled = location.scheduled;
    if (url === current) return;
    if (mode === "push") {
      window.history.pushState(null, "", url);
    } else {
      window.history.replaceState(null, "", url);
    }
  }

  private syncFromUrl(): void {
    const { location, corrected } = parseLocation(
      window.location.pathname,
      window.location.search,
      this.chat,
    );
    this.chat = location.chat;
    this.settings = location.settings;
    this.workbench = location.workbench;
    this.scheduled = location.scheduled;
    if (corrected) window.history.replaceState(null, "", formatLocation(location));
  }
}

export const router = new Router();
