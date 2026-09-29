// The URL is the source of truth for where the user is. Components read the
// location from here and navigate through here; the layout derives its state
// from it rather than mounting a component per route, so every transition
// plays the same whether it came from a click or the browser's back button.
//
// History holds places, not panel states: opening a session or settings
// pushes an entry, toggling the workspace replaces the current one, so back
// moves between places the user visited.
//
// The router also owns which agent is current. Changing it points API calls
// at the new agent and rebinds the agent connection (see `paths.ts`).

import {
  formatLocation,
  MAIN_CHAT,
  parseLocation,
  type AppLocation,
  type ChatLocation,
  type SettingsLocation,
  type SettingsScope,
  type TeamPage,
  type WorkbenchLocation,
} from "./routes";
import { readLastAgent, rememberLastAgent, setCurrentAgent } from "./paths";
import { defaultSection, isSectionOf, type SettingsSection } from "./settings-sections";

type HistoryMode = "push" | "replace";

class Router {
  agent = $state<string | null>(null);
  chat = $state<ChatLocation>(MAIN_CHAT);
  settings = $state<SettingsLocation | null>(null);
  workbench = $state<WorkbenchLocation | null>(null);
  scheduled = $state<boolean>(false);
  team = $state<TeamPage | null>(null);

  private started = false;

  /** Read the current URL and follow back/forward from then on. */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.syncFromUrl();
    window.addEventListener("popstate", () => this.syncFromUrl());
  }

  /** The current location, as one value. */
  private current(): AppLocation {
    return {
      agent: this.agent,
      chat: this.chat,
      settings: this.settings,
      workbench: this.workbench,
      scheduled: this.scheduled,
      team: this.team,
    };
  }

  /** The chat side, with everything else closed. */
  private chatSide(chat: ChatLocation): AppLocation {
    return {
      agent: this.agent,
      chat,
      settings: null,
      workbench: null,
      scheduled: false,
      team: null,
    };
  }

  /**
   * Switch to another agent, on the same kind of page when it exists for
   * every agent (its settings, its scheduled view), else on its main chat.
   */
  openAgent(name: string): void {
    // Already on this agent's own page; from a team page it still navigates.
    const onAgentPage =
      this.team === null && this.workbench === null && this.settings?.scope !== "hub";
    if (name === this.agent && onAgentPage) return;
    rememberLastAgent(name);
    this.go(
      {
        agent: name,
        chat: { runId: null, workspace: this.chat.workspace },
        settings: this.settings?.scope === "agent" ? this.settings : null,
        workbench: null,
        scheduled: this.scheduled,
        team: null,
      },
      "push",
    );
  }

  /** Show a run in the main pane, leaving settings or the workbench if open. */
  openSession(runId: string): void {
    this.go(this.chatSide({ ...this.chat, runId }), "push");
  }

  /** Return the main pane to the main chat, leaving settings or the workbench if open. */
  openMainChat(): void {
    this.go(this.chatSide({ ...this.chat, runId: null }), "push");
  }

  /**
   * Point the current place at another run without adding history, for when
   * the shown session continues in a new run.
   */
  replaceSession(runId: string): void {
    this.go({ ...this.current(), chat: { ...this.chat, runId } }, "replace");
  }

  /**
   * Open or close the workspace panel. From settings or the workbench this is
   * a move back to the chat side, so it adds history like any other change of
   * place.
   */
  setWorkspace(open: boolean): void {
    const onChatSide =
      this.settings === null && this.workbench === null && !this.scheduled && this.team === null;
    this.go(this.chatSide({ ...this.chat, workspace: open }), onChatSide ? "replace" : "push");
  }

  /** Open a settings page: an agent's by default, or the hub's. */
  openSettings(section?: SettingsSection, scope: SettingsScope = "agent"): void {
    const target =
      section !== undefined && isSectionOf(scope, section) ? section : defaultSection(scope);
    this.go({ ...this.chatSide(this.chat), settings: { scope, section: target } }, "push");
  }

  /** Leave settings for the chat side as it was before settings opened. */
  closeSettings(): void {
    if (this.settings === null) return;
    this.go(this.chatSide(this.chat), "push");
  }

  /** Open the workbench: an artifact, or the artifact list when `artifact` is null. */
  openWorkbench(artifact: string | null = null): void {
    this.go({ ...this.chatSide(this.chat), workbench: { artifact, full: false } }, "push");
  }

  /**
   * Show the open artifact filling the window, or return it to the Residuum UI.
   * A view mode of the same place, so it replaces history rather than adding.
   */
  setWorkbenchFull(full: boolean): void {
    const artifact = this.workbench?.artifact ?? null;
    if (artifact === null) return;
    this.go({ ...this.chatSide(this.chat), workbench: { artifact, full } }, "replace");
  }

  /** Leave the workbench for the chat side as it was before it opened. */
  closeWorkbench(): void {
    if (this.workbench === null) return;
    this.go(this.chatSide(this.chat), "push");
  }

  /** Open the Scheduled view (pulses and scheduled actions). */
  openScheduled(): void {
    this.go({ ...this.chatSide(this.chat), scheduled: true }, "push");
  }

  /** Leave the Scheduled view for the chat side as it was before it opened. */
  closeScheduled(): void {
    if (!this.scheduled) return;
    this.go(this.chatSide(this.chat), "push");
  }

  /** Open a team page: the overview of every agent, or the shared files. */
  openTeam(page: TeamPage = "overview"): void {
    this.go({ ...this.chatSide(this.chat), team: page }, "push");
  }

  /** Leave the team pages for the chat side as it was before they opened. */
  closeTeam(): void {
    if (this.team === null) return;
    this.go(this.chatSide(this.chat), "push");
  }

  /**
   * Settle on an agent that exists, once the list of agents is known. Fills in
   * the agent when the URL named none (`/`), and moves off an agent that isn't
   * in the list (a deleted agent, a mistyped URL) to the last-used one, else
   * the first. Leaves the location alone when it is fine, or when there are no
   * agents at all.
   */
  resolveAgent(names: readonly string[]): void {
    const first = names[0];
    if (first === undefined) return;
    if (this.agent !== null && names.includes(this.agent)) {
      rememberLastAgent(this.agent);
      return;
    }
    const last = readLastAgent();
    const target = last !== null && names.includes(last) ? last : first;
    rememberLastAgent(target);
    const onAgentPage =
      this.team === null && this.workbench === null && this.settings?.scope !== "hub";
    this.go(
      onAgentPage
        ? {
            agent: target,
            chat: MAIN_CHAT,
            settings: null,
            workbench: null,
            scheduled: false,
            team: null,
          }
        : { ...this.current(), agent: target, chat: MAIN_CHAT },
      "replace",
    );
  }

  private apply(location: AppLocation): void {
    this.agent = location.agent;
    this.chat = location.chat;
    this.settings = location.settings;
    this.workbench = location.workbench;
    this.scheduled = location.scheduled;
    this.team = location.team;
    setCurrentAgent(location.agent);
  }

  private go(location: AppLocation, mode: HistoryMode): void {
    const url = formatLocation(location);
    const current = `${window.location.pathname}${window.location.search}`;
    this.apply(location);
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
      { agent: this.agent, chat: this.chat, fallbackAgent: readLastAgent() },
    );
    this.apply(location);
    if (corrected) window.history.replaceState(null, "", formatLocation(location));
  }
}

export const router = new Router();
