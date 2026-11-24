import { landingNotices } from "./landing-notices";
import { notifications } from "./notifications.svelte";
import { readLastAgent, rememberLastAgent } from "./paths";
import {
  entryAfterPush,
  entryAfterReplace,
  overlayEntryAfter,
  panelAddress,
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

export interface PlaceExtras {
  panel?: Panel;
  settings?: SettingsTarget;
}

export interface OverlayHandle {
  close: () => void;
}

interface OverlayRecord {
  id: string;
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

type ClosableParam = "panel" | "settings" | "item";

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
      return place.kind === "inbox" && (place.thread !== null || place.compose !== null)
        ? { ...location, place: { ...place, thread: null, compose: null } }
        : null;
  }
}

function byName(a: string, b: string): number {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

class Router {
  location = $state.raw<AppLocation>(locationAt(HOME));
  boundAgent = $state<string | null>(null);
  readonly guard = new NavigationGuard();

  viewedAgent = $derived(viewedAgentOf(this.location.place));

  private started = false;
  private overlaysOnly = false;
  private entry = $state.raw<EntryState>({ idx: 0 });
  private lastViewed: string | null = null;
  private known: ReadonlySet<string> | null = null;
  private lastUsed: string | null = null;
  private waiting: { pathname: string; search: string } | null = null;
  private overlays: OverlayRecord[] = [];
  private overlayCount = 0;
  private queued = 0;
  private traversals: Promise<void> = Promise.resolve();
  private settleTraversal: (() => void) | null = null;
  private stopGeneration = 0;

  start(): void {
    if (this.started) return;
    this.started = true;
    this.lastViewed = readLastAgent();
    window.addEventListener("popstate", this.onPopState);
    window.addEventListener("beforeunload", this.guard.onBeforeUnload);
    this.entry = withoutOverlay(readEntry(window.history.state) ?? { idx: 0 });
    this.applyUrl(this.readUrl(), this.entry);
  }

  startForOverlays(): void {
    if (this.started) return;
    this.started = true;
    this.overlaysOnly = true;
    window.addEventListener("popstate", this.onPopState);
    this.entry = { idx: 0 };
    window.history.replaceState(this.entry, "", currentUrl());
  }

  stop(): void {
    this.stopGeneration += 1;
    this.settleTraversal?.();
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

  openPlace(place: Place, extras: PlaceExtras = {}): Promise<boolean> {
    return this.go(
      { place, panel: extras.panel ?? null, settings: extras.settings ?? null },
      "push",
    );
  }

  open(location: AppLocation): Promise<boolean> {
    return this.go(location, "push");
  }

  replacePlace(place: Place, extras: PlaceExtras = {}): Promise<boolean> {
    return this.go(
      { place, panel: extras.panel ?? null, settings: extras.settings ?? null },
      "replace",
    );
  }

  openPanel(panel: Panel): Promise<boolean> {
    return this.go({ ...this.location, panel }, "push");
  }

  replacePanel(panel: Panel): Promise<boolean> {
    return this.go({ ...this.location, panel }, "replace");
  }

  closePanel(): Promise<boolean> {
    return this.close("panel");
  }

  openSettings(target: SettingsTarget): Promise<boolean> {
    return this.go({ ...this.location, settings: target }, "push");
  }

  openSettingsSection(section: SectionId): Promise<boolean> {
    return this.changeSettings((settings) => ({ ...settings, section }), "push");
  }

  switchSettingsSection(section: SectionId): Promise<boolean> {
    return this.changeSettings((settings) => ({ ...settings, section }), "replace");
  }

  switchSettingsScope(scope: string): Promise<boolean> {
    return this.changeSettings(
      (settings) => ({
        scope,
        section: sectionAfterScopeSwitch(settings.section, scopeKind(scope)),
      }),
      "replace",
    );
  }

  closeSettings(): Promise<boolean> {
    return this.close("settings");
  }

  closeSettingsSection(): Promise<boolean> {
    if (this.location.settings?.section == null) return Promise.resolve(true);
    if (this.queued > 0) return Promise.resolve(false);
    const steps = stepsToClose(this.entry, this.entry.section);
    if (steps === null)
      return this.changeSettings((open) => ({ ...open, section: null }), "replace");
    return this.traverse(-steps).then(() => true);
  }

  closeItem(): Promise<boolean> {
    return this.close("item");
  }

  get canReturnToPanel(): boolean {
    const { backUrl } = this.entry;
    return (
      backUrl !== undefined &&
      this.location.panel !== null &&
      backUrl !== panelAddress(this.location)
    );
  }

  /**
   * Go Back to the panel this one replaced. It is the browser's Back, so
   * unsaved work is asked about the way Back asks.
   */
  returnToPanel(): void {
    if (this.canReturnToPanel && this.queued === 0) window.history.back();
  }

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
    if (this.queued > 0) void this.traversals.then(push);
    else push();
    return {
      close: () => {
        this.closeOverlay(record);
      },
    };
  }

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

  resolveArtifacts(names: readonly string[]): void {
    this.correct(correctForArtifacts(this.location, new Set(names)));
  }

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

  private afterAsking(): Promise<void> {
    return this.traversals;
  }

  private commit(target: AppLocation, mode: HistoryMode): void {
    this.waiting = null;
    const url = formatLocation(target);
    const from = this.location;
    const onOverlay = mode === "push" && this.entry.overlay !== undefined;
    let entry: EntryState;
    if (onOverlay) {
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

  private close(param: ClosableParam): Promise<boolean> {
    const target = withoutParam(this.location, param);
    if (target === null) return Promise.resolve(true);
    if (this.queued > 0) return Promise.resolve(false);
    const leave = (): Promise<boolean> => {
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
    if (at < 0) return;
    this.overlays.splice(at, 1);
    if (this.entry.overlay === record.id) void this.traverse(-1);
  }

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

  private traverse(delta: number): Promise<void> {
    this.queued += 1;
    const queuedUnderGeneration = this.stopGeneration;
    const run = (): Promise<void> =>
      new Promise<void>((resolve) => {
        if (this.stopGeneration !== queuedUnderGeneration) {
          resolve();
          return;
        }
        // Captured now: the timer may fire after a test environment has torn
        // down the global `window`, and settling must not read it then.
        const view: Window = window;
        const finish = (): void => {
          view.clearTimeout(timer);
          this.settleTraversal = null;
          resolve();
        };
        const timer = view.setTimeout(finish, TRAVERSAL_TIMEOUT_MS);
        this.settleTraversal = finish;
        view.history.go(delta);
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
      if (landed.idx < this.entry.idx) {
        this.entry = landed;
        void this.traverse(-1);
        return;
      }
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
    const { search } = window.location;
    const parsed = parseUrl(window.location.pathname, search, { lastUsed: this.lastUsed });
    return {
      ...parsed,
      notices: [...parsed.notices, ...landingNotices(search, parsed.location)],
    };
  }

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
