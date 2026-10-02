// The router: the URL is the source of truth for where the user is. This
// holds the current location, navigates by push or replace, and keeps
// the history rules that make Back behave: closing a panel or modal is
// `history.back()` when this page pushed what opened it, overlays own an entry
// so Back closes them, and unsaved work is asked about before it is lost.
//
// Stores never import this: they expose data and commands, and views navigate.
//
// The router also publishes the bound agent, the agent whose connection the
// app keeps open (see `viewed-agent.ts`).

import { notifications } from "./notifications.svelte";
import { readLastAgent, rememberLastAgent } from "./paths";
import {
  entryAfterPush,
  entryAfterReplace,
  overlayEntryAfter,
  readEntry,
  sameEntry,
  stepsToClose,
  withoutOverlay,
  type EntryState,
} from "./history-entry";
import { NavigationGuard } from "./navigation-guard";
import {
  correctForAgents,
  correctForArtifacts,
  formatLocation,
  HOME,
  locationAt,
  locationsEqual,
  panelAllowed,
  parseUrl,
  viewedAgentOf,
  type AppLocation,
  type Panel,
  type ParsedUrl,
  type Place,
  type SettingsTarget,
} from "./routes";
import {
  defaultSection,
  isSectionOf,
  scopeKind,
  sectionAfterScopeSwitch,
  type SectionId,
} from "./settings-sections";
import { setViewedAgent } from "./viewed-agent";

type HistoryMode = "push" | "replace";

/** What may ride along with a place: a panel, or the Settings modal. */
export interface PlaceExtras {
  panel?: Panel;
  settings?: SettingsTarget;
}

/** An open overlay's hold on a history entry. */
export interface OverlayHandle {
  /** Close the overlay from the UI: pops its entry, so Back doesn't land on a stale one. */
  close: () => void;
}

interface OverlayRecord {
  id: string;
  /** The history entry the overlay holds; null until it is pushed. */
  idx: number | null;
  closed: boolean;
  onDismiss: () => void;
}

/**
 * How long a traversal the router started may go without the browser
 * reporting it. A `history.go` past the ends of the stack does nothing and
 * never reports, and the next traversal mustn't wait on it forever.
 */
const TRAVERSAL_TIMEOUT_MS = 1000;

function currentUrl(): string {
  return `${window.location.pathname}${window.location.search}`;
}

/**
 * What closing removes from the URL: the panel, the Settings modal, or the
 * item open in the Inbox or the artifact selected on the Workbench.
 */
type ClosableParam = "panel" | "settings" | "item";

/** `location` with `param` closed, or null when it isn't open. */
function withoutParam(location: AppLocation, param: ClosableParam): AppLocation | null {
  const { place } = location;
  switch (param) {
    case "panel":
      return location.panel === null ? null : { ...location, panel: null };
    case "settings":
      return location.settings === null ? null : { ...location, settings: null };
    case "item":
      if (place.kind === "workbench") {
        return place.artifact === null
          ? null
          : { ...location, place: { ...place, artifact: null } };
      }
      return place.kind === "inbox" && place.item !== null
        ? { ...location, place: { ...place, item: null } }
        : null;
  }
}

function byName(a: string, b: string): number {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

class Router {
  /** Where the user is: the place, its context panel and the Settings modal. */
  location = $state.raw<AppLocation>(locationAt(HOME));
  /**
   * The bound agent: the viewed agent, or on a place with none, the agent most
   * recently viewed. The agent connection follows it.
   */
  boundAgent = $state<string | null>(null);
  /** The unsaved-edit guard: views register what leaving would lose, and the app says how to ask. */
  readonly guard = new NavigationGuard();

  /** The agent the place belongs to, or null on Home, Inbox, the Workbench and Shared files. */
  viewedAgent = $derived(viewedAgentOf(this.location.place));

  private started = false;
  /** Started for overlay entries only, on a page outside the app's routes: the URL is left alone. */
  private overlaysOnly = false;
  /** The marks of the history entry being shown. */
  private entry: EntryState = { idx: 0 };
  /** The agent most recently viewed, for places that view none. */
  private lastViewed: string | null = null;
  /** Every agent's name once the list is known; null before. */
  private known: ReadonlySet<string> | null = null;
  /** The last-used agent, once the list is known. */
  private lastUsed: string | null = null;
  /** The URL that resolves under the last-used agent, waiting for the list. */
  private waiting: { pathname: string; search: string } | null = null;
  private overlays: OverlayRecord[] = [];
  private overlayCount = 0;
  /** Traversals this router started that haven't landed, and the queue that runs them one at a time. */
  private queued = 0;
  private traversals: Promise<void> = Promise.resolve();
  /** Settles the traversal in flight; set while the router waits for the browser to report it. */
  private settleTraversal: (() => void) | null = null;

  /** Read the current URL and follow the browser's history from then on. */
  start(): void {
    if (this.started) return;
    this.started = true;
    this.lastViewed = readLastAgent();
    window.addEventListener("popstate", this.onPopState);
    window.addEventListener("beforeunload", this.guard.onBeforeUnload);
    // An overlay doesn't survive a reload, so its entry is an ordinary one now.
    this.entry = withoutOverlay(readEntry(window.history.state) ?? { idx: 0 });
    this.applyUrl(this.readUrl(), this.entry);
  }

  /**
   * Follow the history for overlay entries only, on a page outside the app's
   * routes (the primitives gallery): Back closes its overlays, and the address
   * is left as it is instead of being read as a place.
   */
  startForOverlays(): void {
    if (this.started) return;
    this.started = true;
    this.overlaysOnly = true;
    window.addEventListener("popstate", this.onPopState);
    this.entry = { idx: 0 };
    window.history.replaceState(this.entry, "", currentUrl());
  }

  /** Stop following the browser's history. */
  stop(): void {
    if (!this.started) return;
    this.started = false;
    this.overlaysOnly = false;
    window.removeEventListener("popstate", this.onPopState);
    window.removeEventListener("beforeunload", this.guard.onBeforeUnload);
  }

  get place(): Place {
    return this.location.place;
  }

  get panel(): Panel | null {
    return this.location.panel;
  }

  get settings(): SettingsTarget | null {
    return this.location.settings;
  }

  // ── Navigation ─────────────────────────────────────────────────────
  //
  // Each returns whether the navigation happened: false when the user
  // declined to leave unsaved work. It happens before the call returns unless
  // the guard has something to ask, or a traversal of the history is landing.

  /** Open a place (push), with a panel or the Settings modal if given. */
  openPlace(place: Place, extras: PlaceExtras = {}): Promise<boolean> {
    return this.go(
      { place, panel: extras.panel ?? null, settings: extras.settings ?? null },
      "push",
    );
  }

  /** Go to a place without adding history, for moves the user didn't make by navigating. */
  replacePlace(place: Place, extras: PlaceExtras = {}): Promise<boolean> {
    return this.go(
      { place, panel: extras.panel ?? null, settings: extras.settings ?? null },
      "replace",
    );
  }

  /** Open a session or file in the context panel from outside the panel (push). */
  openPanel(panel: Panel): Promise<boolean> {
    return this.go({ ...this.location, panel }, "push");
  }

  /** Change what the panel shows from inside it (replace): another file, or a session's next run. */
  replacePanel(panel: Panel): Promise<boolean> {
    return this.go({ ...this.location, panel }, "replace");
  }

  /** Close the panel: `history.back()` when this page pushed the entry that opened it, else a replace without it. */
  closePanel(): Promise<boolean> {
    return this.close("panel");
  }

  /** Open the Settings modal (push). */
  openSettings(target: SettingsTarget): Promise<boolean> {
    return this.go({ ...this.location, settings: target }, "push");
  }

  /** Open a section from the phone's section list (push). */
  openSettingsSection(section: SectionId): Promise<boolean> {
    return this.changeSettings((settings) => ({ ...settings, section }), "push");
  }

  /** Switch sections at medium and wide widths (replace). */
  switchSettingsSection(section: SectionId): Promise<boolean> {
    return this.changeSettings((settings) => ({ ...settings, section }), "replace");
  }

  /** Switch the modal's scope (replace), keeping the section when the new scope has it. */
  switchSettingsScope(scope: string): Promise<boolean> {
    return this.changeSettings(
      (settings) => ({
        scope,
        section: sectionAfterScopeSwitch(settings.section, scopeKind(scope)),
      }),
      "replace",
    );
  }

  /** Close the Settings modal: `history.back()` when this page pushed the entry that opened it, else a replace without it. */
  closeSettings(): Promise<boolean> {
    return this.close("settings");
  }

  /** Back from a section to the phone's section list: `history.back()` when this page pushed the section, else a replace without it. */
  closeSettingsSection(): Promise<boolean> {
    if (this.location.settings?.section == null) return Promise.resolve(true);
    if (this.queued > 0) return Promise.resolve(false);
    const steps = stepsToClose(this.entry, this.entry.section);
    if (steps === null)
      return this.changeSettings((open) => ({ ...open, section: null }), "replace");
    return this.traverse(-steps).then(() => true);
  }

  /**
   * Close the item open in the Inbox, or collapse the artifact selected on the
   * Workbench: `history.back()` when this page pushed the entry that opened it,
   * else a replace without it.
   */
  closeItem(): Promise<boolean> {
    return this.close("item");
  }

  /**
   * Hold a history entry for an overlay that is open, so Back
   * closes the overlay before it leaves the place. `onDismiss` runs when the
   * entry is left by anything but the returned handle: Back, or navigating
   * elsewhere. The overlay closes itself through the handle, which pops the
   * entry.
   */
  openOverlay(onDismiss: () => void): OverlayHandle {
    this.overlayCount += 1;
    const record: OverlayRecord = {
      id: `overlay-${String(this.overlayCount)}`,
      idx: null,
      closed: false,
      onDismiss,
    };
    const push = (): void => {
      if (record.closed) return;
      const entry = overlayEntryAfter(this.entry, record.id);
      window.history.pushState(entry, "", currentUrl());
      this.entry = entry;
      record.idx = entry.idx;
      this.overlays.push(record);
    };
    // A traversal still landing would carry the new entry off with it.
    if (this.queued > 0) void this.traversals.then(push);
    else push();
    return {
      close: () => {
        this.closeOverlay(record);
      },
    };
  }

  /**
   * Settle on agents once their list is known: corrects a location on an agent
   * that doesn't exist, resolves a URL that was waiting for the last-used agent,
   * and moves the bound agent off one that is gone. An empty list changes nothing
   * (the setup wizard shows).
   */
  setKnownAgents(names: readonly string[]): void {
    const first = [...names].sort(byName)[0];
    if (first === undefined) return;
    const known = new Set(names);
    this.known = known;
    const stored = readLastAgent();
    this.lastUsed = stored !== null && known.has(stored) ? stored : first;
    if (this.lastViewed === null || !known.has(this.lastViewed)) this.lastViewed = this.lastUsed;

    if (this.waiting !== null) {
      const { pathname, search } = this.waiting;
      this.waiting = null;
      this.applyUrl(parseUrl(pathname, search, { lastUsed: this.lastUsed }), this.entry);
    } else {
      this.correct(correctForAgents(this.location, known));
    }
    const viewed = this.viewedAgent;
    if (viewed !== null && known.has(viewed)) rememberLastAgent(viewed);
    this.publish();
  }

  /** Once the artifact list has loaded, move the Workbench off an artifact that isn't in it. */
  resolveArtifacts(names: readonly string[]): void {
    this.correct(correctForArtifacts(this.location, new Set(names)));
  }

  // ── Internals ──────────────────────────────────────────────────────

  /** A location the place and modal can show: a panel that can't show there is left out, and so is an unknown section. */
  private normalize(target: AppLocation): AppLocation {
    let { panel, settings } = target;
    if (panel !== null && !panelAllowed(target.place, panel)) panel = null;
    if (settings?.section != null) {
      const kind = scopeKind(settings.scope);
      if (!isSectionOf(kind, settings.section)) {
        settings = { scope: settings.scope, section: defaultSection(kind) };
      }
    }
    return { place: target.place, panel, settings };
  }

  private changeSettings(
    change: (settings: SettingsTarget) => SettingsTarget,
    mode: HistoryMode,
  ): Promise<boolean> {
    const { settings } = this.location;
    if (settings === null) return Promise.resolve(false);
    return this.go({ ...this.location, settings: change(settings) }, mode);
  }

  private go(requested: AppLocation, mode: HistoryMode): Promise<boolean> {
    // A traversal is landing: go from where it leaves the history.
    if (this.queued > 0) return this.traversals.then(() => this.go(requested, mode));
    const target = this.normalize(requested);
    if (locationsEqual(target, this.location)) return Promise.resolve(true);
    const losses = this.guard.losses(target);
    if (losses.length === 0) {
      this.commit(target, mode);
      return Promise.resolve(true);
    }
    return this.guard.ask(losses).then(async (allowed) => {
      if (!allowed) return false;
      await this.afterAsking();
      this.commit(target, mode);
      return true;
    });
  }

  /**
   * Asking opens the confirm dialog, an overlay with its own history entry,
   * and closing it goes back over that entry. Go on from where that leaves
   * the history, or the navigation would land on the dialog's entry and be
   * undone when the browser reports going back.
   */
  private afterAsking(): Promise<void> {
    return this.traversals;
  }

  /** Write `target` into the history and show it. */
  private commit(target: AppLocation, mode: HistoryMode): void {
    this.waiting = null;
    const url = formatLocation(target);
    const from = this.location;
    const onOverlay = mode === "push" && this.entry.overlay !== undefined;
    let entry: EntryState;
    if (onOverlay) {
      // Leaving while an overlay is open: the navigation takes the overlay's
      // entry, so the overlay isn't left behind in the history.
      entry = entryAfterPush(
        withoutOverlay({ ...this.entry, idx: this.entry.idx - 1 }),
        from,
        target,
      );
      window.history.replaceState(entry, "", url);
    } else if (mode === "push") {
      entry = entryAfterPush(this.entry, from, target);
      window.history.pushState(entry, "", url);
    } else {
      entry = entryAfterReplace(this.entry, from, target);
      window.history.replaceState(entry, "", url);
    }
    this.entry = entry;
    const viewed = viewedAgentOf(target.place);
    if (viewed !== null) rememberLastAgent(viewed);
    this.show(target);
    if (mode === "push") this.dismissOverlaysAbove(null);
  }

  /** Close `param` by going back to before it opened, or by replacing it away when the page didn't open it. */
  private close(param: ClosableParam): Promise<boolean> {
    const target = withoutParam(this.location, param);
    if (target === null) return Promise.resolve(true);
    // A traversal is already on its way; a second one would go back past it.
    if (this.queued > 0) return Promise.resolve(false);
    const leave = (): Promise<boolean> => {
      // Worked out when leaving, since asking pushes and pops the confirm dialog's entry.
      const steps = stepsToClose(this.entry, this.entry[param]);
      if (steps === null) {
        this.commit(target, "replace");
        return Promise.resolve(true);
      }
      return this.traverse(-steps).then(() => true);
    };
    const losses = this.guard.losses(target);
    if (losses.length === 0) return leave();
    return this.guard.ask(losses).then(async (allowed) => {
      if (!allowed) return false;
      await this.afterAsking();
      return leave();
    });
  }

  private closeOverlay(record: OverlayRecord): void {
    if (record.closed) return;
    record.closed = true;
    const at = this.overlays.indexOf(record);
    // Not pushed yet: there's no entry to pop.
    if (at < 0) return;
    this.overlays.splice(at, 1);
    if (this.entry.overlay === record.id) void this.traverse(-1);
  }

  /** Dismiss the overlays whose entries are above `idx` (all of them for null), topmost first. */
  private dismissOverlaysAbove(idx: number | null): void {
    const gone = this.overlays.filter(
      (overlay) => idx === null || (overlay.idx !== null && overlay.idx > idx),
    );
    if (gone.length === 0) return;
    this.overlays = this.overlays.filter((overlay) => !gone.includes(overlay));
    for (const overlay of gone.reverse()) {
      overlay.closed = true;
      overlay.onDismiss();
    }
  }

  /** Go `delta` entries through the history. Resolves once the browser reports arriving. */
  private traverse(delta: number): Promise<void> {
    this.queued += 1;
    const run = (): Promise<void> =>
      new Promise<void>((resolve) => {
        const finish = (): void => {
          window.clearTimeout(timer);
          this.settleTraversal = null;
          resolve();
        };
        const timer = window.setTimeout(finish, TRAVERSAL_TIMEOUT_MS);
        this.settleTraversal = finish;
        window.history.go(delta);
      });
    this.traversals = this.traversals.then(run).then(() => {
      this.queued -= 1;
    });
    return this.traversals;
  }

  private readonly onPopState = (event: PopStateEvent): void => {
    const ours = this.settleTraversal !== null;
    this.settleTraversal?.();
    let landed = readEntry(event.state);
    const parsed = this.readUrl();

    const overlayId = landed?.overlay;
    if (
      landed !== null &&
      overlayId !== undefined &&
      !this.overlays.some((o) => o.id === overlayId)
    ) {
      // An entry whose overlay is already closed. Going back, step over it.
      if (landed.idx < this.entry.idx) {
        this.entry = landed;
        void this.traverse(-1);
        return;
      }
      // Going forward, it is an ordinary entry from here on.
      landed = withoutOverlay(landed);
      window.history.replaceState(landed, "", currentUrl());
    }
    this.dismissOverlaysAbove(landed?.idx ?? null);
    if (this.overlaysOnly) {
      this.entry = landed ?? { idx: this.entry.idx - 1 };
      return;
    }

    const arrived = landed ?? { idx: this.entry.idx - 1 };
    const leaving =
      ours || parsed.needsLastUsed || locationsEqual(parsed.location, this.location)
        ? []
        : this.guard.losses(parsed.location);
    if (leaving.length > 0) {
      void this.askBeforeLeaving(leaving, arrived);
      return;
    }
    this.entry = arrived;
    this.applyUrl(parsed, arrived);
  };

  /**
   * Back or Forward can't be cancelled, so put the location the user was on
   * back on top of the history, ask, and go to where they were headed only if
   * they confirm (it is the entry just below).
   */
  private async askBeforeLeaving(losses: string[], arrived: EntryState): Promise<void> {
    const shown = this.location;
    const restored: EntryState = { idx: arrived.idx + 1 };
    if (shown.settings !== null && this.entry.settings !== undefined) {
      restored.settings = Math.min(this.entry.settings, restored.idx);
    }
    if (shown.settings?.section != null && this.entry.section !== undefined) {
      restored.section = Math.min(this.entry.section, restored.idx);
    }
    if (shown.panel !== null && this.entry.panel !== undefined) {
      restored.panel = Math.min(this.entry.panel, restored.idx);
    }
    if (this.entry.item !== undefined) restored.item = Math.min(this.entry.item, restored.idx);
    window.history.pushState(restored, "", formatLocation(shown));
    this.entry = restored;
    if (await this.guard.ask(losses)) await this.traverse(-1);
  }

  private readUrl(): ParsedUrl {
    return parseUrl(window.location.pathname, window.location.search, { lastUsed: this.lastUsed });
  }

  /**
   * Show what the address bar says, correcting the address (by replace) to the
   * canonical form of where it leads. A URL that waits for the last-used agent
   * keeps the address as it is until the agent list is known.
   */
  private applyUrl(parsed: ParsedUrl, arrived: EntryState): void {
    if (parsed.needsLastUsed) {
      this.waiting = { pathname: window.location.pathname, search: window.location.search };
      this.entry = arrived;
      this.show(parsed.location);
      return;
    }
    let { location } = parsed;
    const notices = [...parsed.notices];
    if (this.known !== null) {
      const corrected = correctForAgents(location, this.known);
      location = corrected.location;
      notices.push(...corrected.notices);
    }
    const entry = entryAfterReplace(arrived, parsed.location, location);
    const url = formatLocation(location);
    if (url !== currentUrl() || !sameEntry(readEntry(window.history.state), entry)) {
      window.history.replaceState(entry, "", url);
    }
    this.entry = entry;
    this.show(location);
    for (const notice of notices) notifications.surface("notice", notice);
  }

  /** Apply a correction to the location shown, by replace, telling the user. */
  private correct(result: { location: AppLocation; notices: string[] }): void {
    if (result.notices.length === 0) return;
    this.commit(result.location, "replace");
    for (const notice of result.notices) notifications.surface("notice", notice);
  }

  private show(location: AppLocation): void {
    this.location = location;
    const viewed = viewedAgentOf(location.place);
    if (viewed !== null) this.lastViewed = viewed;
    this.publish();
  }

  private publish(): void {
    this.boundAgent = this.viewedAgent ?? this.lastViewed;
    setViewedAgent(this.boundAgent);
  }
}

export const router = new Router();
