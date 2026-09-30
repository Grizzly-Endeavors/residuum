// The current app's navigation, on the router. The views that predate the new
// shell think in their own places (the chat side with a session or the
// workspace open, a settings page, the workbench, the scheduled view, the team
// pages), so this reads those off the router's location and turns their
// commands into router navigations. It goes away with the views that use it.

import {
  ALL_SCOPE,
  defaultSection,
  legacyHostSection,
  scopeKind,
  sectionFromOldName,
  type ScopeKind,
} from "./settings-sections";
import type { LegacyScope, LegacySection } from "./legacy-settings-sections";
import { router } from "./router.svelte";
import { HOME, isAgentPlace, type Panel, type Place } from "./routes";

/** What the chat side of the app shows. */
export interface LegacyChat {
  /** The run shown in the main pane, or null for the main chat. */
  runId: string | null;
  workspace: boolean;
}

export interface LegacySettings {
  scope: LegacyScope;
  section: LegacySection;
}

export type LegacyTeamPage = "overview" | "files";

export type LegacyView =
  | "chat"
  | "workspace"
  | "settings"
  | "hub-settings"
  | "workbench"
  | "scheduled"
  | "team"
  | "team-files";

class LegacyRouter {
  /** The old section the user picked, which stands in for the new section it maps to. */
  private picked = $state<LegacySection | null>(null);
  /** The artifact shown filling the window. A view mode of the old page, not part of the URL. */
  private fullArtifact = $state<string | null>(null);

  /** The agent the old views work with: the bound agent. */
  get agent(): string | null {
    return router.boundAgent;
  }

  get chat(): LegacyChat {
    const { place, panel } = router.location;
    const inSession =
      panel?.kind === "session" && isAgentPlace(place) && panel.agent === place.agent;
    return { runId: inSession ? panel.runId : null, workspace: place.kind === "files" };
  }

  /** The settings page, with the old section that hosts the new section the URL names. */
  get settings(): LegacySettings | null {
    const target = router.settings;
    if (target === null) return null;
    const kind = scopeKind(target.scope);
    const section = target.section ?? defaultSection(kind);
    const host = legacyHostSection(kind, section);
    const picked = this.picked;
    if (picked !== null && sectionFromOldName(picked, host.scope)?.section === section) {
      return { scope: host.scope, section: picked };
    }
    return host;
  }

  /** The agent the settings page edits: the one its scope names. Null for the install's settings. */
  get settingsAgent(): string | null {
    const scope = router.settings?.scope;
    return scope === undefined || scope === ALL_SCOPE ? null : scope;
  }

  get workbench(): { artifact: string | null; full: boolean } | null {
    const { place } = router;
    if (place.kind !== "workbench") return null;
    return {
      artifact: place.artifact,
      full: place.artifact !== null && place.artifact === this.fullArtifact,
    };
  }

  get scheduled(): boolean {
    return router.place.kind === "schedule";
  }

  get team(): LegacyTeamPage | null {
    const { kind } = router.place;
    if (kind === "home") return "overview";
    return kind === "shared-files" ? "files" : null;
  }

  get inbox(): boolean {
    return router.place.kind === "inbox";
  }

  /** Whether the sessions list is the place: the old sessions sidebar is how it shows. */
  get activity(): boolean {
    return router.place.kind === "activity";
  }

  /** Which of the old views fills the window. */
  get view(): LegacyView {
    if (this.settings !== null) return this.settings.scope === "hub" ? "hub-settings" : "settings";
    if (this.workbench !== null) return "workbench";
    if (this.team === "overview") return "team";
    if (this.team === "files") return "team-files";
    if (this.scheduled) return "scheduled";
    return this.chat.workspace ? "workspace" : "chat";
  }

  // ── Commands ───────────────────────────────────────────────────────

  /**
   * Switch to another agent, on the same kind of page when it exists for every
   * agent (its files, schedule or settings), else on its main chat.
   */
  openAgent(name: string): void {
    const { place, settings } = router;
    if (isAgentPlace(place) && place.agent === name && settings === null) return;
    const kind = isAgentPlace(place) && place.kind !== "activity" ? place.kind : "chat";
    const carried =
      settings !== null && scopeKind(settings.scope) === "agent"
        ? { settings: { scope: name, section: settings.section } }
        : {};
    void router.openPlace({ kind, agent: name }, carried);
  }

  /** Show a run in the main pane, leaving settings, the workbench or the team pages. */
  openSession(runId: string): void {
    const agent = router.boundAgent;
    if (agent === null) return;
    const place = this.chatSide(agent);
    void router.openPlace(place, { panel: { kind: "session", agent, runId } });
  }

  /** Return the main pane to the main chat, leaving settings, the workbench or the team pages. */
  openMainChat(): void {
    const agent = router.boundAgent;
    void router.openPlace(agent === null ? HOME : this.chatSide(agent));
  }

  /** Point the open session at another run, for when it continues in a new one. */
  replaceSession(runId: string): void {
    const { place, panel } = router.location;
    if (panel?.kind !== "session" || !isAgentPlace(place)) return;
    void router.replacePanel({ kind: "session", agent: panel.agent, runId });
  }

  /** Open or close the workspace beside the main pane. */
  setWorkspace(open: boolean): void {
    const agent = router.boundAgent;
    if (agent === null) return;
    const panel = this.sessionPanel(agent);
    const place: Place = open ? { kind: "files", agent } : { kind: "chat", agent };
    void router.openPlace(place, panel === null ? {} : { panel });
  }

  /** Open a settings page: an agent's by default, or the install's. */
  openSettings(section?: LegacySection, scope: LegacyScope = "agent"): void {
    const found = section === undefined ? null : sectionFromOldName(section, scope);
    const kind: ScopeKind = found?.scope ?? (scope === "hub" ? "all" : "agent");
    const agent = router.boundAgent;
    this.picked = section ?? null;
    if (kind === "agent" && agent !== null) {
      void router.openSettings({ scope: agent, section: found?.section ?? null });
    } else {
      void router.openSettings({ scope: ALL_SCOPE, section: found?.section ?? null });
    }
  }

  /** Switch the settings page to another old section. */
  selectSettingsSection(section: LegacySection): void {
    const current = this.settings;
    if (current === null) return;
    const found = sectionFromOldName(section, current.scope);
    if (found === null) return;
    this.picked = section;
    void router.switchSettingsSection(found.section);
  }

  closeSettings(): void {
    void router.closeSettings();
  }

  openWorkbench(artifact: string | null = null): void {
    this.fullArtifact = null;
    void router.openPlace({ kind: "workbench", artifact });
  }

  /** Show the open artifact filling the window, or return it to the Residuum UI. */
  setWorkbenchFull(full: boolean): void {
    const artifact = this.workbench?.artifact ?? null;
    this.fullArtifact = full ? artifact : null;
  }

  closeWorkbench(): void {
    this.openMainChat();
  }

  openScheduled(): void {
    const agent = router.boundAgent;
    if (agent !== null) void router.openPlace({ kind: "schedule", agent });
  }

  closeScheduled(): void {
    this.openMainChat();
  }

  openTeam(page: LegacyTeamPage = "overview"): void {
    void router.openPlace(page === "files" ? { kind: "shared-files" } : HOME);
  }

  closeTeam(): void {
    this.openMainChat();
  }

  openInbox(): void {
    void router.openPlace({ kind: "inbox", agent: null, tab: "active", item: null });
  }

  closeInbox(): void {
    this.openMainChat();
  }

  /** The place the chat side is on: the agent's files when they are open, else its chat. */
  private chatSide(agent: string): Place {
    const { place } = router;
    const open = isAgentPlace(place) && place.agent === agent && place.kind === "files";
    return open ? place : { kind: "chat", agent };
  }

  /** The session shown for `agent`, to keep it when the workspace opens or closes. */
  private sessionPanel(agent: string): Panel | null {
    const { panel } = router;
    return panel?.kind === "session" && panel.agent === agent ? panel : null;
  }
}

export const legacyRouter = new LegacyRouter();
