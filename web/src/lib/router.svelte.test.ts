// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EntryState } from "./history-entry";
import type { AppLocation, Panel, Place } from "./routes";

import type { router as routerInstance } from "./router.svelte";

type Router = typeof routerInstance;

interface Harness {
  router: Router;
  /** Toasts the router raised. */
  notices: string[];
  /** The bound agent, as published to the socket coordinator. */
  viewed: (string | null)[];
  /** The URLs of entries pushed, and of entries replaced, since boot. */
  pushes: string[];
  replaces: string[];
  url: () => string;
  /** The marks on the entry being shown. */
  entry: () => EntryState;
  shown: () => string;
}

const started: Router[] = [];

/**
 * jsdom reports a traversal after a chain of up to three zero-delay timers, each
 * queued by the one before, and a guarded Back or a closing modal can start a
 * second traversal behind the first. Zero-delay timers run in the order they
 * were queued however long the event loop stalls, so waiting in turns of the
 * same timer lets every one of them run first. A wait measured in milliseconds
 * can end between two links of the chain once the machine is busy, and the
 * traversal then lands after the assertion.
 */
const SETTLE_TURNS = 12;

/** Let the browser report traversals and the router finish what they start. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < SETTLE_TURNS; turn += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const chat = (agent: string): Place => ({ kind: "chat", agent });
const files = (agent: string): Place => ({ kind: "files", agent });
const home: Place = { kind: "home" };
const run = (agent: string, runId: string): Panel => ({ kind: "session", agent, runId });

/** A fresh router on a page that opened at `url`. `lastAgent` is what local storage remembers. */
async function boot(url: string, lastAgent: string | null = null): Promise<Harness> {
  vi.restoreAllMocks();
  vi.resetModules();
  localStorage.clear();
  if (lastAgent !== null) localStorage.setItem("residuum-last-agent", lastAgent);
  window.history.replaceState(null, "", url);

  const { router } = await import("./router.svelte");
  const { notifications } = await import("./notifications.svelte");
  const { onViewedAgentChange } = await import("./viewed-agent");
  const notices: string[] = [];
  vi.spyOn(notifications, "surface").mockImplementation((_kind, message) => {
    notices.push(message);
  });
  const viewed: (string | null)[] = [];
  onViewedAgentChange((agent) => viewed.push(agent));
  const pushes: string[] = [];
  const replaces: string[] = [];
  const push = window.history.pushState.bind(window.history);
  const replace = window.history.replaceState.bind(window.history);
  vi.spyOn(window.history, "pushState").mockImplementation((state, title, target) => {
    pushes.push(String(target));
    push(state, title, target);
  });
  // Replacing an entry's state under the same URL (stamping its marks) isn't a navigation.
  vi.spyOn(window.history, "replaceState").mockImplementation((state, title, target) => {
    if (String(target) !== `${window.location.pathname}${window.location.search}`) {
      replaces.push(String(target));
    }
    replace(state, title, target);
  });
  router.start();
  started.push(router);
  return {
    router,
    notices,
    viewed,
    pushes,
    replaces,
    url: () => `${window.location.pathname}${window.location.search}`,
    entry: () => window.history.state as EntryState,
    shown: () => locationUrl(router.location),
  };
}

function locationUrl(location: AppLocation): string {
  const panel = location.panel === null ? "" : ` panel=${JSON.stringify(location.panel)}`;
  const settings =
    location.settings === null ? "" : ` settings=${JSON.stringify(location.settings)}`;
  return `${JSON.stringify(location.place)}${panel}${settings}`;
}

/**
 * Asks the way the confirm dialog does: it is an overlay with its own history
 * entry, and the answer comes once the dialog has closed through its handle.
 */
function confirmOnOverlay(router: Router, answer: boolean) {
  return (_losses: readonly string[]): Promise<boolean> =>
    new Promise((resolve) => {
      const handle = router.openOverlay(() => {
        resolve(false);
      });
      setTimeout(() => {
        handle.close();
        resolve(answer);
      }, 0);
    });
}

afterEach(() => {
  for (const router of started.splice(0)) router.stop();
  vi.restoreAllMocks();
});

describe("opening the app", () => {
  it("sends / to /home by replace, writing the entry's marks", async () => {
    const page = await boot("/");
    expect(page.url()).toBe("/home");
    expect(page.replaces).toEqual(["/home"]);
    expect(page.pushes).toEqual([]);
    expect(page.router.place).toEqual(home);
    expect(page.entry().idx).toBe(0);
  });

  it("leaves a canonical URL alone", async () => {
    const page = await boot("/agent/scout/files?panel=file:SOUL.md");
    expect(page.replaces).toEqual([]);
    expect(page.router.place).toEqual(files("scout"));
    expect(page.router.panel).toEqual({ kind: "file", path: "SOUL.md" });
  });

  it.each([
    ["/agent/scout/sessions/run-1?workspace", "/agent/scout?panel=session:scout:run-1"],
    ["/agent/scout/workspace", "/agent/scout/files"],
    ["/agent/scout/scheduled", "/agent/scout/schedule"],
    ["/agent/scout/settings/providers", "/agent/scout?settings=scout/model"],
    ["/team/settings/cloud", "/home?settings=_all/cloud"],
    ["/team/workbench/tip-splitter?full", "/team/workbench/tip-splitter"],
    ["/workbench", "/team/workbench"],
    ["/team", "/home"],
    ["/nowhere", "/home"],
  ])("redirects %s to %s by replace", async (from, to) => {
    const page = await boot(from);
    expect(page.url()).toBe(to);
    expect(page.replaces).toEqual([to]);
    expect(page.pushes).toEqual([]);
  });

  it("tells the user about a settings scope that can't exist", async () => {
    const page = await boot("/home?settings=Bad_Name/model");
    expect(page.url()).toBe("/home?settings=_all/general");
    expect(page.notices).toEqual([`There's no agent named "Bad_Name".`]);
  });

  it("keeps an old URL that resolves under the last-used agent as it is until the agents are known", async () => {
    const page = await boot("/settings/memory");
    expect(page.url()).toBe("/settings/memory");
    expect(page.replaces).toEqual([]);
    expect(page.router.place).toEqual(home);

    page.router.setKnownAgents(["scout", "atlas"]);
    expect(page.url()).toBe("/agent/atlas?settings=atlas/memory");
    expect(page.replaces).toEqual(["/agent/atlas?settings=atlas/memory"]);
    expect(page.router.viewedAgent).toBe("atlas");
  });

  it("resolves under the remembered agent when it still exists", async () => {
    const page = await boot("/scheduled", "scout");
    page.router.setKnownAgents(["atlas", "scout"]);
    expect(page.url()).toBe("/agent/scout/schedule");
  });

  it("resolves under the first agent by name when the remembered one is gone", async () => {
    const page = await boot("/sessions/run-1", "gone");
    page.router.setKnownAgents(["scout", "atlas"]);
    expect(page.url()).toBe("/agent/atlas?panel=session:atlas:run-1");
  });

  it("sends a macOS notification's Open link to the last-used agent's Files", async () => {
    const page = await boot("/notification/abc123", "scout");
    page.router.setKnownAgents(["atlas", "scout"]);
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
  });

  it("does nothing on an empty agent list", async () => {
    const page = await boot("/scheduled");
    page.router.setKnownAgents([]);
    expect(page.url()).toBe("/scheduled");
  });
});

describe("agents that don't exist", () => {
  it("sends an unknown agent place to Home with a toast, by replace", async () => {
    const page = await boot("/agent/ghost/files");
    expect(page.router.place).toEqual(files("ghost"));
    page.router.setKnownAgents(["scout"]);
    expect(page.url()).toBe("/home");
    expect(page.replaces).toEqual(["/home"]);
    expect(page.notices).toEqual([`There's no agent named "ghost".`]);
  });

  it("sends an unknown settings scope to All agents", async () => {
    const page = await boot("/home?settings=ghost/model");
    page.router.setKnownAgents(["scout"]);
    expect(page.url()).toBe("/home?settings=_all/general");
    expect(page.notices).toEqual([`There's no agent named "ghost".`]);
  });

  it("follows an agent that is deleted while viewed", async () => {
    const page = await boot("/agent/scout");
    page.router.setKnownAgents(["scout", "atlas"]);
    expect(page.notices).toEqual([]);
    page.router.setKnownAgents(["atlas"]);
    expect(page.url()).toBe("/home");
    expect(page.notices).toEqual([`There's no agent named "scout".`]);
    expect(page.router.boundAgent).toBe("atlas");
  });

  it("moves the Workbench off an artifact that isn't in the loaded list", async () => {
    const page = await boot("/team/workbench/ghost");
    page.router.resolveArtifacts(["tip-splitter"]);
    expect(page.url()).toBe("/team/workbench");
    expect(page.replaces).toContain("/team/workbench");
    expect(page.notices).toEqual([`There's no workbench page named "ghost".`]);
  });
});

describe("the bound agent", () => {
  it("is the viewed agent on an agent place, and is published", async () => {
    const page = await boot("/agent/scout");
    expect(page.router.viewedAgent).toBe("scout");
    expect(page.router.boundAgent).toBe("scout");
    expect(page.viewed).toEqual(["scout"]);
    await page.router.openPlace(chat("atlas"));
    expect(page.viewed).toEqual(["scout", "atlas"]);
  });

  it("stays on the most recently viewed agent on places with none", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(home);
    expect(page.router.viewedAgent).toBeNull();
    expect(page.router.boundAgent).toBe("scout");
    await page.router.openPlace({ kind: "shared-files" });
    await page.router.openPlace({ kind: "workbench", artifact: null });
    expect(page.router.boundAgent).toBe("scout");
    expect(page.viewed).toEqual(["scout"]);
  });

  it("is the remembered agent on Home before any agent was viewed", async () => {
    const page = await boot("/home", "atlas");
    expect(page.router.viewedAgent).toBeNull();
    expect(page.router.boundAgent).toBe("atlas");
    expect(page.viewed).toEqual(["atlas"]);
  });

  it("moves off a remembered agent that no longer exists", async () => {
    const page = await boot("/home", "gone");
    page.router.setKnownAgents(["scout", "atlas"]);
    expect(page.router.boundAgent).toBe("atlas");
    expect(page.viewed).toEqual(["gone", "atlas"]);
  });

  it("remembers an agent once it is known to exist, not before", async () => {
    const page = await boot("/agent/atlas", "scout");
    expect(localStorage.getItem("residuum-last-agent")).toBe("scout");
    page.router.setKnownAgents(["atlas", "scout"]);
    expect(localStorage.getItem("residuum-last-agent")).toBe("atlas");
  });

  it("remembers the agent the user navigates to", async () => {
    const page = await boot("/home");
    await page.router.openPlace(chat("atlas"));
    expect(localStorage.getItem("residuum-last-agent")).toBe("atlas");
  });
});

describe("push and replace", () => {
  it("pushes when opening a place", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    await page.router.openPlace({ kind: "workbench", artifact: "tip-splitter" });
    expect(page.pushes).toEqual(["/agent/scout/files", "/team/workbench/tip-splitter"]);
    expect(page.replaces).toEqual([]);
    expect(page.entry().idx).toBe(2);
  });

  it("replaces when a move isn't the user's navigation", async () => {
    const page = await boot("/agent/scout");
    await page.router.replacePlace(files("scout"));
    expect(page.pushes).toEqual([]);
    expect(page.replaces).toEqual(["/agent/scout/files"]);
    expect(page.entry().idx).toBe(0);
  });

  it("does nothing when asked for where it already is", async () => {
    const page = await boot("/agent/scout");
    await expect(page.router.openPlace(chat("scout"))).resolves.toBe(true);
    expect(page.pushes).toEqual([]);
  });

  it("pushes when opening a session or file in the panel from elsewhere", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPanel(run("scout", "run-1"));
    await page.router.openPanel({ kind: "file", path: "SOUL.md" });
    expect(page.pushes).toEqual([
      "/agent/scout?panel=session:scout:run-1",
      "/agent/scout?panel=file:SOUL.md",
    ]);
  });

  it("replaces when the panel switches what it shows, from inside", async () => {
    const page = await boot("/agent/scout?panel=file:a.md");
    await page.router.replacePanel({ kind: "file", path: "b.md" });
    await page.router.replacePanel(run("scout", "run-2"));
    expect(page.pushes).toEqual([]);
    expect(page.replaces).toEqual([
      "/agent/scout?panel=file:b.md",
      "/agent/scout?panel=session:scout:run-2",
    ]);
  });

  it("pushes when opening the Settings modal, and a section from the phone list", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: null });
    await page.router.openSettingsSection("memory");
    expect(page.pushes).toEqual([
      "/agent/scout?settings=scout",
      "/agent/scout?settings=scout/memory",
    ]);
  });

  it("replaces when switching sections at wider widths, and scopes in the modal", async () => {
    const page = await boot("/agent/scout?settings=scout/model");
    await page.router.switchSettingsSection("memory");
    await page.router.switchSettingsScope("atlas");
    await page.router.switchSettingsScope("_all");
    expect(page.pushes).toEqual([]);
    expect(page.replaces).toEqual([
      "/agent/scout?settings=scout/memory",
      "/agent/scout?settings=atlas/memory",
      "/agent/scout?settings=_all/general",
    ]);
  });

  it("keeps the section across a scope switch when the new scope has it", async () => {
    const page = await boot("/home?settings=scout/raw");
    await page.router.switchSettingsScope("_all");
    expect(page.router.settings).toEqual({ scope: "_all", section: "raw" });
    await page.router.switchSettingsScope("atlas");
    expect(page.router.settings).toEqual({ scope: "atlas", section: "raw" });
  });

  it("does nothing to the modal when it isn't open", async () => {
    const page = await boot("/agent/scout");
    await expect(page.router.switchSettingsSection("memory")).resolves.toBe(false);
    await expect(page.router.openSettingsSection("memory")).resolves.toBe(false);
    await expect(page.router.switchSettingsScope("atlas")).resolves.toBe(false);
    expect(page.pushes).toEqual([]);
    expect(page.replaces).toEqual([]);
  });

  it("drops the panel and the modal when opening another place", async () => {
    const page = await boot("/agent/scout?panel=size&settings=scout/model");
    await page.router.openPlace(files("scout"));
    expect(page.url()).toBe("/agent/scout/files");
  });

  it("carries a panel or the modal when asked to", async () => {
    const page = await boot("/home");
    await page.router.openPlace(chat("scout"), { panel: run("scout", "run-1") });
    expect(page.url()).toBe("/agent/scout?panel=session:scout:run-1");
    await page.router.openPlace(home, { settings: { scope: "_all", section: "cloud" } });
    expect(page.url()).toBe("/home?settings=_all/cloud");
  });

  it("leaves out a panel its place can't show", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPanel(run("atlas", "run-1"));
    expect(page.url()).toBe("/agent/scout");
    await page.router.openPlace(home, { panel: { kind: "size" } });
    expect(page.url()).toBe("/home");
  });

  it("uses the scope's default section for one the scope doesn't have", async () => {
    const page = await boot("/home");
    await page.router.openSettings({ scope: "_all", section: "model" });
    expect(page.url()).toBe("/home?settings=_all/general");
  });
});

describe("the item open in the Inbox", () => {
  type InboxPlace = Extract<Place, { kind: "inbox" }>;
  const list: InboxPlace = { kind: "inbox", agent: null, tab: "active", item: null };
  const opened = (id: string): InboxPlace => ({ ...list, item: { agent: "atlas", id } });

  it("closes by going back when this page opened it, so Back never lands on a copy of the list", async () => {
    const page = await boot("/inbox");
    await page.router.openPlace(opened("a"));
    expect(page.url()).toBe("/inbox?item=atlas:a");
    await expect(page.router.closeInboxItem()).resolves.toBe(true);
    expect(page.url()).toBe("/inbox");
    expect(page.entry().idx).toBe(0);
    expect(page.replaces).toEqual([]);
  });

  it("closes by replace when the page was linked to it", async () => {
    const page = await boot("/inbox?item=atlas:a");
    await page.router.closeInboxItem();
    expect(page.url()).toBe("/inbox");
    expect(page.replaces).toEqual(["/inbox"]);
    expect(page.pushes).toEqual([]);
  });

  it("keeps the opener when another item is switched to, so closing leaves the list as it was", async () => {
    const page = await boot("/inbox");
    await page.router.openPlace(opened("a"));
    await page.router.replacePlace(opened("b"));
    expect(page.url()).toBe("/inbox?item=atlas:b");
    await page.router.closeInboxItem();
    expect(page.url()).toBe("/inbox");
    expect(page.entry().idx).toBe(0);
  });

  it("forgets the opener when the filter changes", async () => {
    const page = await boot("/inbox");
    await page.router.openPlace(opened("a"));
    await page.router.replacePlace({ ...opened("a"), tab: "archived" });
    expect(page.entry().item).toBeUndefined();
    await page.router.closeInboxItem();
    expect(page.url()).toBe("/inbox?tab=archived");
    expect(page.entry().idx).toBe(1);
  });

  it("has nothing to close with no item open", async () => {
    const page = await boot("/inbox");
    await expect(page.router.closeInboxItem()).resolves.toBe(true);
    expect(page.url()).toBe("/inbox");
    expect(page.pushes).toEqual([]);
  });
});

describe("closing the panel and the modal", () => {
  it("goes back when this page pushed the entry that opened the modal", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    expect(page.entry().idx).toBe(1);
    await expect(page.router.closeSettings()).resolves.toBe(true);
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.settings).toBeNull();
    // Back, not a new entry: nothing more was pushed or replaced.
    expect(page.pushes).toEqual(["/agent/scout?settings=scout/model"]);
    expect(page.replaces).toEqual([]);
    expect(page.entry().idx).toBe(0);
  });

  it("replaces the parameter away when the page was deep-linked", async () => {
    const page = await boot("/agent/scout?settings=scout/model");
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
    expect(page.replaces).toEqual(["/agent/scout"]);
    expect(page.pushes).toEqual([]);
    expect(page.entry().idx).toBe(0);
  });

  it("goes back past every entry the phone flow pushed", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: null });
    await page.router.openSettingsSection("memory");
    expect(page.entry().idx).toBe(2);
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(0);
  });

  it("goes back one step from the section list, and Back from a section lands on the list", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: null });
    await page.router.openSettingsSection("memory");
    window.history.back();
    await settle();
    expect(page.router.settings).toEqual({ scope: "scout", section: null });
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
  });

  it("closes the panel the same way: back when pushed, replace when linked", async () => {
    const pushed = await boot("/agent/scout");
    await pushed.router.openPanel({ kind: "file", path: "a.md" });
    await pushed.router.closePanel();
    expect(pushed.url()).toBe("/agent/scout");
    expect(pushed.entry().idx).toBe(0);
    expect(pushed.replaces).toEqual([]);

    const linked = await boot("/agent/scout?panel=size");
    await linked.router.closePanel();
    expect(linked.url()).toBe("/agent/scout");
    expect(linked.replaces).toEqual(["/agent/scout"]);
  });

  it("goes back past every panel entry opened from elsewhere in a row", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPanel({ kind: "file", path: "a.md" });
    await page.router.openPanel({ kind: "file", path: "b.md" });
    await page.router.closePanel();
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(0);
  });

  it("leaves the panel open when closing a modal opened over it, then closes the panel", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPanel({ kind: "size" });
    await page.router.openSettings({ scope: "scout", section: "model" });
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout?panel=size");
    await page.router.closePanel();
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(0);
  });

  it("closes a modal that opened along with its place by leaving the place where it is", async () => {
    const page = await boot("/home");
    await page.router.openPlace(chat("scout"), { settings: { scope: "scout", section: "model" } });
    expect(page.entry().settings).toBeUndefined();
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.place).toEqual(chat("scout"));
    expect(page.replaces).toEqual(["/agent/scout"]);
    expect(page.entry().idx).toBe(1);
  });

  it("keeps the marks when the modal's section or scope changes by replace", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    await page.router.switchSettingsSection("memory");
    await page.router.switchSettingsScope("_all");
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(0);
  });

  it("goes back only once when closed twice before the browser lands", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    await page.router.openSettings({ scope: "scout", section: "model" });
    const first = page.router.closeSettings();
    await expect(page.router.closeSettings()).resolves.toBe(false);
    await first;
    await settle();
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.entry().idx).toBe(1);
  });

  it("treats closing what isn't open as done", async () => {
    const page = await boot("/agent/scout");
    await expect(page.router.closePanel()).resolves.toBe(true);
    await expect(page.router.closeSettings()).resolves.toBe(true);
    expect(page.pushes).toEqual([]);
    expect(page.replaces).toEqual([]);
  });

  it("follows Back and Forward through the modal", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    window.history.back();
    await settle();
    expect(page.router.settings).toBeNull();
    expect(page.url()).toBe("/agent/scout");
    window.history.forward();
    await settle();
    expect(page.router.settings).toEqual({ scope: "scout", section: "model" });
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
  });

  it("still closes by going back after a reload at the entry that opened it", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    page.router.stop();
    // A reload keeps the entry's state and starts a new router on it.
    const reloaded = await bootOnCurrentEntry();
    expect(reloaded.router.settings).toEqual({ scope: "scout", section: "model" });
    await reloaded.router.closeSettings();
    expect(reloaded.url()).toBe("/agent/scout");
    expect(reloaded.entry().idx).toBe(0);
  });
});

/** A new router on whatever entry the page is on, as after a reload. */
async function bootOnCurrentEntry(): Promise<Harness> {
  const { router } = await import("./router.svelte");
  router.stop();
  vi.resetModules();
  const fresh = await import("./router.svelte");
  const { notifications } = await import("./notifications.svelte");
  vi.spyOn(notifications, "surface").mockImplementation(() => {});
  fresh.router.start();
  started.push(fresh.router);
  return {
    router: fresh.router,
    notices: [],
    viewed: [],
    pushes: [],
    replaces: [],
    url: () => `${window.location.pathname}${window.location.search}`,
    entry: () => window.history.state as EntryState,
    shown: () => locationUrl(fresh.router.location),
  };
}

describe("overlay entries", () => {
  it("pushes an entry with the same URL, marked as the overlay's", async () => {
    const page = await boot("/agent/scout");
    page.router.openOverlay(() => {});
    expect(page.pushes).toEqual(["/agent/scout"]);
    expect(page.entry().overlay).toBeDefined();
    expect(page.entry().idx).toBe(1);
    expect(page.url()).toBe("/agent/scout");
  });

  it("dismisses the overlay on Back, without leaving the place", async () => {
    const page = await boot("/agent/scout/files");
    const dismissed = vi.fn();
    page.router.openOverlay(dismissed);
    window.history.back();
    await settle();
    expect(dismissed).toHaveBeenCalledTimes(1);
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
    expect(page.entry().idx).toBe(0);
  });

  it("pops its entry when the UI closes it, without dismissing it again", async () => {
    const page = await boot("/agent/scout");
    const dismissed = vi.fn();
    const handle = page.router.openOverlay(dismissed);
    handle.close();
    await settle();
    expect(page.entry().idx).toBe(0);
    expect(page.entry().overlay).toBeUndefined();
    expect(dismissed).not.toHaveBeenCalled();
    handle.close();
    await settle();
    expect(page.entry().idx).toBe(0);
  });

  it("stacks: Back closes the top overlay, then the next", async () => {
    const page = await boot("/agent/scout");
    const order: string[] = [];
    page.router.openOverlay(() => order.push("first"));
    page.router.openOverlay(() => order.push("second"));
    window.history.back();
    await settle();
    expect(order).toEqual(["second"]);
    window.history.back();
    await settle();
    expect(order).toEqual(["second", "first"]);
    expect(page.entry().idx).toBe(0);
  });

  it("takes the overlay's entry when a navigation leaves it open, and dismisses it", async () => {
    const page = await boot("/agent/scout");
    const dismissed = vi.fn();
    const handle = page.router.openOverlay(dismissed);
    await page.router.openPlace(files("scout"));
    // The navigation replaced the overlay's entry: one entry deeper, not two.
    expect(page.replaces).toEqual(["/agent/scout/files"]);
    expect(page.entry().idx).toBe(1);
    expect(dismissed).toHaveBeenCalledTimes(1);
    handle.close();
    await settle();
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.entry().idx).toBe(1);
    window.history.back();
    await settle();
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.place).toEqual(chat("scout"));
  });

  it("navigates from where a closing overlay leaves the history", async () => {
    const page = await boot("/agent/scout");
    const handle = page.router.openOverlay(() => {});
    handle.close();
    const navigated = page.router.openPlace(files("scout"));
    await navigated;
    await settle();
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
    expect(page.entry().idx).toBe(1);
    window.history.back();
    await settle();
    expect(page.router.place).toEqual(chat("scout"));
  });

  it("opens another overlay once a closing one has left the history", async () => {
    const page = await boot("/agent/scout");
    const first = page.router.openOverlay(() => {});
    first.close();
    const dismissed = vi.fn();
    page.router.openOverlay(dismissed);
    await settle();
    expect(page.entry().idx).toBe(1);
    expect(page.entry().overlay).toBeDefined();
    window.history.back();
    await settle();
    expect(dismissed).toHaveBeenCalledTimes(1);
    expect(page.entry().idx).toBe(0);
  });

  it("never pushes an overlay that was closed before its entry could be", async () => {
    const page = await boot("/agent/scout");
    page.router.openOverlay(() => {}).close();
    const second = page.router.openOverlay(() => {});
    second.close();
    await settle();
    expect(page.entry().idx).toBe(0);
  });

  it("steps over the entry of an overlay closed out of order", async () => {
    const page = await boot("/agent/scout");
    const below = page.router.openOverlay(() => {});
    const dismissedTop = vi.fn();
    page.router.openOverlay(dismissedTop);
    // The lower overlay closes first and leaves its entry behind.
    below.close();
    window.history.back();
    await settle();
    expect(dismissedTop).toHaveBeenCalledTimes(1);
    // Back from the top overlay landed on the stale entry and went on past it.
    expect(page.entry().idx).toBe(0);
    expect(page.entry().overlay).toBeUndefined();
  });

  it("treats the entry of a closed overlay as an ordinary one when Forward returns to it", async () => {
    const page = await boot("/agent/scout");
    const dismissed = vi.fn();
    page.router.openOverlay(dismissed);
    window.history.back();
    await settle();
    window.history.forward();
    await settle();
    expect(dismissed).toHaveBeenCalledTimes(1);
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(1);
    expect(page.entry().overlay).toBeUndefined();
  });

  it("closes the modal and the overlay over it together when the modal closes by going back", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    const dismissed = vi.fn();
    page.router.openOverlay(dismissed);
    await page.router.closeSettings();
    expect(dismissed).toHaveBeenCalledTimes(1);
    expect(page.url()).toBe("/agent/scout");
    expect(page.entry().idx).toBe(0);
  });

  it("starts on an ordinary entry after a reload on an overlay's entry", async () => {
    const page = await boot("/agent/scout");
    page.router.openOverlay(() => {});
    page.router.stop();
    const reloaded = await bootOnCurrentEntry();
    expect(reloaded.entry().overlay).toBeUndefined();
    expect(reloaded.url()).toBe("/agent/scout");
  });
});

describe("the unsaved-edit guard", () => {
  it("navigates without asking when nothing would be lost", async () => {
    const page = await boot("/agent/scout");
    const confirm = vi.fn(() => Promise.resolve(true));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register(() => null);
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(true);
    expect(confirm).not.toHaveBeenCalled();
    expect(page.url()).toBe("/agent/scout/files");
  });

  it("asks what would be lost before an in-app navigation, and goes on when confirmed", async () => {
    const page = await boot("/agent/scout");
    const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(true));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register(() => "unsaved changes to SOUL.md");
    page.router.guard.register(() => "staged settings changes");
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(true);
    expect(confirm).toHaveBeenCalledWith(["unsaved changes to SOUL.md", "staged settings changes"]);
    expect(page.url()).toBe("/agent/scout/files");
  });

  it("stays put when the user declines", async () => {
    const page = await boot("/agent/scout");
    page.router.guard.setConfirm(() => Promise.resolve(false));
    page.router.guard.register(() => "unsaved changes");
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(false);
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.place).toEqual(chat("scout"));
    expect(page.pushes).toEqual([]);
  });

  it("refuses to lose work when nothing can ask", async () => {
    const page = await boot("/agent/scout");
    page.router.guard.register(() => "unsaved changes");
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(false);
    expect(page.url()).toBe("/agent/scout");
  });

  it("gives each check the place being navigated to, so it can tell what it keeps", async () => {
    const page = await boot("/agent/scout/files");
    const seen: (AppLocation | null)[] = [];
    page.router.guard.setConfirm(() => Promise.resolve(true));
    page.router.guard.register((target) => {
      seen.push(target);
      return target?.place.kind === "files" ? null : "an edit";
    });
    await page.router.openPlace(files("scout"), { panel: { kind: "file", path: "a.md" } });
    await page.router.openPlace(chat("scout"));
    expect(seen.map((target) => target?.place.kind)).toEqual(["files", "chat"]);
  });

  it("asks before closing the panel", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPanel({ kind: "file", path: "a.md" });
    const confirm = vi.fn(() => Promise.resolve(false));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register((target) => (target?.panel === null ? "an edit" : null));
    await expect(page.router.closePanel()).resolves.toBe(false);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(page.router.panel).toEqual({ kind: "file", path: "a.md" });
    confirm.mockResolvedValue(true);
    await expect(page.router.closePanel()).resolves.toBe(true);
    expect(page.url()).toBe("/agent/scout");
  });

  it("re-pushes the location on Back, asks, and stays when the user declines", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    page.router.guard.setConfirm(() => Promise.resolve(false));
    page.router.guard.register(() => "unsaved changes");
    window.history.back();
    await settle();
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
    // The location was put back on top of the history.
    expect(page.pushes).toEqual(["/agent/scout/files", "/agent/scout/files"]);
    expect(page.entry().idx).toBe(1);
  });

  it("goes to where Back was headed when the user confirms", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(true));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register(() => "unsaved changes");
    window.history.back();
    await settle();
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.place).toEqual(chat("scout"));
    expect(page.entry().idx).toBe(0);
  });

  it("guards Forward the same way", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    window.history.back();
    await settle();
    const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(false));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register(() => "unsaved changes");
    window.history.forward();
    await settle();
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.place).toEqual(chat("scout"));
  });

  it("doesn't ask when Back lands on the same location, like closing an overlay", async () => {
    const page = await boot("/agent/scout");
    const confirm = vi.fn(() => Promise.resolve(true));
    page.router.guard.setConfirm(confirm);
    page.router.guard.register(() => "unsaved changes");
    page.router.openOverlay(() => {});
    window.history.back();
    await settle();
    expect(confirm).not.toHaveBeenCalled();
    expect(page.url()).toBe("/agent/scout");
  });

  it("keeps the modal closable by going back after a guarded Back was declined", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    page.router.guard.setConfirm(() => Promise.resolve(false));
    page.router.guard.register((target) => (target?.settings === null ? "staged changes" : null));
    window.history.back();
    await settle();
    expect(page.router.settings).toEqual({ scope: "scout", section: "model" });
    page.router.guard.setConfirm(() => Promise.resolve(true));
    await page.router.closeSettings();
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.settings).toBeNull();
  });

  it("uses the browser's own prompt for reload and close, only when work would be lost", async () => {
    const page = await boot("/agent/scout");
    const clean = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(clean);
    expect(clean.defaultPrevented).toBe(false);

    const seen: (AppLocation | null)[] = [];
    page.router.guard.register((target) => {
      seen.push(target);
      return "unsaved changes";
    });
    const dirty = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(dirty);
    expect(dirty.defaultPrevented).toBe(true);
    expect(seen).toEqual([null]);
  });

  it("stops asking about a check that was unregistered", async () => {
    const page = await boot("/agent/scout");
    const stop = page.router.guard.register(() => "unsaved changes");
    stop();
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(true);
  });

  it("navigates from where the confirm dialog's own entry leaves the history", async () => {
    const page = await boot("/agent/scout");
    page.router.guard.setConfirm(confirmOnOverlay(page.router, true));
    page.router.guard.register(() => "unsaved changes");
    await expect(page.router.openPlace(files("scout"))).resolves.toBe(true);
    await settle();
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
    expect(page.entry()).toEqual({ idx: 1 });
    window.history.back();
    await vi.waitFor(() => {
      expect(page.router.place).toEqual(chat("scout"));
    });
  });

  it("closes the modal from where the confirm dialog's entry leaves the history", async () => {
    const page = await boot("/agent/scout");
    await page.router.openSettings({ scope: "scout", section: "model" });
    page.router.guard.setConfirm(confirmOnOverlay(page.router, true));
    page.router.guard.register((target) => (target?.settings === null ? "staged changes" : null));
    await expect(page.router.closeSettings()).resolves.toBe(true);
    await settle();
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.settings).toBeNull();
    expect(page.entry().idx).toBe(0);
  });

  it("closes a deep-linked modal by replace once the confirm dialog's entry is gone", async () => {
    const page = await boot("/agent/scout?settings=scout/model");
    page.router.guard.setConfirm(confirmOnOverlay(page.router, true));
    page.router.guard.register((target) => (target?.settings === null ? "staged changes" : null));
    await expect(page.router.closeSettings()).resolves.toBe(true);
    await settle();
    expect(page.url()).toBe("/agent/scout");
    expect(page.router.settings).toBeNull();
    expect(page.entry()).toEqual({ idx: 0 });
  });

  it("goes where Back was headed after a confirm dialog with its own entry", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    page.router.guard.setConfirm(confirmOnOverlay(page.router, true));
    page.router.guard.register(() => "unsaved changes");
    window.history.back();
    await vi.waitFor(() => {
      expect(page.router.place).toEqual(chat("scout"));
      expect(page.url()).toBe("/agent/scout");
      expect(page.entry().idx).toBe(0);
    });
  });
});

describe("following overlay entries only", () => {
  async function bootOutsideRoutes(url: string): Promise<Router> {
    vi.resetModules();
    window.history.replaceState(null, "", url);
    const { router } = await import("./router.svelte");
    router.startForOverlays();
    started.push(router);
    return router;
  }
  const url = (): string => `${window.location.pathname}${window.location.search}`;

  it("leaves the address alone and marks the entry it starts on", async () => {
    await bootOutsideRoutes("/dev/gallery");
    expect(url()).toBe("/dev/gallery");
    expect(window.history.state).toEqual({ idx: 0 });
  });

  it("closes the topmost overlay on Back, then the next, without leaving the page", async () => {
    const router = await bootOutsideRoutes("/dev/gallery");
    const first = vi.fn();
    const second = vi.fn();
    router.openOverlay(first);
    router.openOverlay(second);
    window.history.back();
    await vi.waitFor(() => {
      expect(second).toHaveBeenCalledTimes(1);
    });
    expect(first).not.toHaveBeenCalled();
    window.history.back();
    await vi.waitFor(() => {
      expect(first).toHaveBeenCalledTimes(1);
    });
    expect(url()).toBe("/dev/gallery");
    expect(window.history.state).toEqual({ idx: 0 });
  });

  it("pops an overlay's entry when the UI closes it", async () => {
    const router = await bootOutsideRoutes("/dev/gallery");
    const dismissed = vi.fn();
    router.openOverlay(dismissed).close();
    await settle();
    expect(dismissed).not.toHaveBeenCalled();
    expect(window.history.state).toEqual({ idx: 0 });
  });
});

describe("following the browser", () => {
  it("shows the place Back lands on, and publishes its agent", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(chat("atlas"));
    window.history.back();
    await settle();
    expect(page.router.place).toEqual(chat("scout"));
    expect(page.viewed).toEqual(["scout", "atlas", "scout"]);
  });

  it("corrects a URL the browser lands on that needs it, by replace", async () => {
    const page = await boot("/agent/scout");
    window.history.pushState(null, "", "/agent/scout?workspace");
    window.dispatchEvent(new PopStateEvent("popstate", { state: null }));
    expect(page.url()).toBe("/agent/scout/files");
    expect(page.router.place).toEqual(files("scout"));
  });

  it("stops following once stopped", async () => {
    const page = await boot("/agent/scout");
    await page.router.openPlace(files("scout"));
    page.router.stop();
    window.history.back();
    await settle();
    expect(page.router.place).toEqual(files("scout"));
  });
});
