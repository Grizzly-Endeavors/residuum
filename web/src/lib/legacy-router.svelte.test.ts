// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { router as routerInstance } from "./router.svelte";
import type { legacyRouter as legacyRouterInstance } from "./legacy-router.svelte";

interface Page {
  router: typeof routerInstance;
  legacy: typeof legacyRouterInstance;
  url: () => string;
}

const started: (typeof routerInstance)[] = [];

/** The current app's navigation on a fresh router that opened at `url`. */
async function boot(url: string): Promise<Page> {
  vi.resetModules();
  localStorage.clear();
  window.history.replaceState(null, "", url);
  const { router } = await import("./router.svelte");
  const { legacyRouter } = await import("./legacy-router.svelte");
  router.start();
  started.push(router);
  return {
    router,
    legacy: legacyRouter,
    url: () => `${window.location.pathname}${window.location.search}`,
  };
}

afterEach(() => {
  for (const router of started.splice(0)) router.stop();
});

describe("which old view fills the window", () => {
  it.each([
    ["/agent/scout", "chat"],
    ["/agent/scout/activity", "chat"],
    ["/inbox", "chat"],
    ["/agent/scout/files", "workspace"],
    ["/agent/scout/schedule", "scheduled"],
    ["/home", "team"],
    ["/team/files", "team-files"],
    ["/team/workbench", "workbench"],
    ["/team/workbench/tip-splitter", "workbench"],
    ["/agent/scout?settings=scout/model", "settings"],
    ["/home?settings=_all/cloud", "hub-settings"],
    ["/team/files?settings=scout", "settings"],
  ])("shows %s as %s", async (url, view) => {
    const { legacy } = await boot(url);
    expect(legacy.view).toBe(view);
  });

  it("reads the chat side off the place and the panel", async () => {
    const { legacy } = await boot("/agent/scout/files?panel=session:scout:run-1");
    expect(legacy.chat).toEqual({ runId: "run-1", workspace: true });
    const chat = await boot("/agent/scout");
    expect(chat.legacy.chat).toEqual({ runId: null, workspace: false });
  });

  it("shows a session only on its own agent's place", async () => {
    const { legacy } = await boot("/team/workbench?panel=session:scout:run-1");
    expect(legacy.chat.runId).toBeNull();
  });

  it("says when the Activity place, the workbench or the inbox is the place", async () => {
    expect((await boot("/agent/scout/activity")).legacy.activity).toBe(true);
    expect((await boot("/inbox")).legacy.inbox).toBe(true);
    expect((await boot("/team/workbench/x")).legacy.workbench).toEqual({
      artifact: "x",
      full: false,
    });
    expect((await boot("/agent/scout")).legacy.workbench).toBeNull();
  });

  it("works with the bound agent", async () => {
    const { legacy, router } = await boot("/agent/scout");
    expect(legacy.agent).toBe("scout");
    await router.openPlace({ kind: "home" });
    expect(legacy.agent).toBe("scout");
  });
});

describe("the settings page", () => {
  it.each([
    ["/agent/scout?settings=scout/model", "agent", "providers"],
    ["/agent/scout?settings=scout/connections", "agent", "channels"],
    ["/agent/scout?settings=scout/tools", "agent", "skills"],
    ["/agent/scout?settings=scout/schedule", "agent", "pulses"],
    ["/agent/scout?settings=scout/servers", "agent", "mcp"],
    ["/agent/scout?settings=scout/runtime", "agent", "runtime"],
    ["/agent/scout?settings=scout/a2a", "agent", "a2a"],
    ["/agent/scout?settings=scout/history", "agent", "history"],
    ["/agent/scout?settings=scout", "agent", "providers"],
    ["/home?settings=_all/general", "hub", "general"],
    ["/home?settings=_all/keys", "hub", "secrets"],
    ["/home?settings=_all/limits", "hub", "sessions"],
    ["/home?settings=_all/diagnostics", "hub", "tracing"],
    ["/home?settings=_all/updates", "hub", "update"],
    ["/home?settings=_all/listener", "hub", "a2a"],
    ["/home?settings=_all", "hub", "general"],
  ])("opens %s on the old %s section %s", async (url, scope, section) => {
    const { legacy } = await boot(url);
    expect(legacy.settings).toEqual({ scope, section });
  });

  it("lands old settings URLs on the right old section", async () => {
    for (const [url, scope, section] of [
      ["/agent/scout/settings/channels", "agent", "channels"],
      ["/agent/scout/settings/skills", "agent", "skills"],
      ["/agent/scout/settings/pulses", "agent", "pulses"],
      ["/agent/scout/settings/mcp", "agent", "mcp"],
      ["/agent/scout/settings/tracing", "hub", "tracing"],
      ["/team/settings/update", "hub", "update"],
      ["/team/settings/agent-keys", "hub", "secrets"],
    ] as const) {
      const { legacy } = await boot(url);
      expect(legacy.settings).toEqual({ scope, section });
    }
  });

  it("names the agent an agent page edits by its scope, not by where the modal is over", async () => {
    const { legacy } = await boot("/agent/scout/files?settings=atlas/memory");
    expect(legacy.settingsAgent).toBe("atlas");
    expect(legacy.settings).toEqual({ scope: "agent", section: "memory" });
    expect((await boot("/home?settings=_all")).legacy.settingsAgent).toBeNull();
    expect((await boot("/agent/scout")).legacy.settingsAgent).toBeNull();
  });

  it("opens the agent's settings from the bound agent, or the install's", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openSettings();
    expect(url()).toBe("/agent/scout?settings=scout");
    legacy.closeSettings();
    await vi.waitFor(() => {
      expect(url()).toBe("/agent/scout");
    });
    legacy.openSettings(undefined, "hub");
    expect(url()).toBe("/agent/scout?settings=_all");
    expect(legacy.view).toBe("hub-settings");
  });

  it("opens a named old section at its new home", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openSettings("webhooks");
    expect(url()).toBe("/agent/scout?settings=scout/connections");
    expect(legacy.settings).toEqual({ scope: "agent", section: "webhooks" });
  });

  it("follows the section the user picks in the old page, and replaces rather than pushes", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openSettings("runtime");
    const depth = window.history.length;
    legacy.selectSettingsSection("skills");
    expect(url()).toBe("/agent/scout?settings=scout/tools");
    expect(legacy.settings).toEqual({ scope: "agent", section: "skills" });
    expect(window.history.length).toBe(depth);
  });

  it("keeps the old section the user picked where two share one new section, and forgets it after", async () => {
    const { legacy } = await boot("/agent/scout");
    legacy.openSettings("runtime");
    legacy.selectSettingsSection("webhooks");
    expect(legacy.settings?.section).toBe("webhooks");
    legacy.selectSettingsSection("channels");
    expect(legacy.settings?.section).toBe("channels");
    legacy.selectSettingsSection("memory");
    expect(legacy.settings?.section).toBe("memory");
    legacy.selectSettingsSection("pulses");
    expect(legacy.settings?.section).toBe("pulses");
  });

  it("picks the hub's secrets or agent keys apart though they share a new section", async () => {
    const { legacy } = await boot("/home");
    legacy.openSettings("agent-keys", "hub");
    expect(legacy.settings).toEqual({ scope: "hub", section: "agent-keys" });
    legacy.selectSettingsSection("secrets");
    expect(legacy.settings).toEqual({ scope: "hub", section: "secrets" });
  });
});

describe("the old commands on the router", () => {
  it("switches agents keeping the kind of page when it exists for every agent", async () => {
    const files = await boot("/agent/scout/files");
    files.legacy.openAgent("atlas");
    expect(files.url()).toBe("/agent/atlas/files");

    const schedule = await boot("/agent/scout/schedule");
    schedule.legacy.openAgent("atlas");
    expect(schedule.url()).toBe("/agent/atlas/schedule");

    const chat = await boot("/agent/scout?panel=session:scout:run-1");
    chat.legacy.openAgent("atlas");
    expect(chat.url()).toBe("/agent/atlas");

    const activity = await boot("/agent/scout/activity");
    activity.legacy.openAgent("atlas");
    expect(activity.url()).toBe("/agent/atlas");
  });

  it("switches agents on the agent settings page, and goes to the agent's chat from a team page", async () => {
    const settings = await boot("/agent/scout?settings=scout/memory");
    settings.legacy.openAgent("atlas");
    expect(settings.url()).toBe("/agent/atlas?settings=atlas/memory");

    const team = await boot("/home");
    team.legacy.openAgent("atlas");
    expect(team.url()).toBe("/agent/atlas");

    const hubSettings = await boot("/agent/scout?settings=_all/cloud");
    hubSettings.legacy.openAgent("atlas");
    expect(hubSettings.url()).toBe("/agent/atlas");
  });

  it("does nothing when that agent's own page is already open", async () => {
    const { legacy } = await boot("/agent/scout");
    const before = window.history.length;
    legacy.openAgent("scout");
    expect(window.history.length).toBe(before);
  });

  it("opens a session in the main pane from the chat, leaving settings and the team pages", async () => {
    const chat = await boot("/agent/scout");
    chat.legacy.openSession("run-1");
    expect(chat.url()).toBe("/agent/scout?panel=session:scout:run-1");
    expect(chat.legacy.chat.runId).toBe("run-1");

    const team = await boot("/agent/scout");
    await team.router.openPlace({ kind: "home" });
    team.legacy.openSession("run-2");
    expect(team.url()).toBe("/agent/scout?panel=session:scout:run-2");

    const settings = await boot("/agent/scout?settings=scout/model");
    settings.legacy.openSession("run-3");
    expect(settings.url()).toBe("/agent/scout?panel=session:scout:run-3");
  });

  it("keeps the workspace open beside a session, and returns to the chat", async () => {
    const { legacy, url } = await boot("/agent/scout/files");
    legacy.openSession("run-1");
    expect(url()).toBe("/agent/scout/files?panel=session:scout:run-1");
    legacy.openMainChat();
    expect(url()).toBe("/agent/scout/files");
  });

  it("points the open session at its next run by replace", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openSession("run-1");
    const before = window.history.length;
    legacy.replaceSession("run-2");
    expect(url()).toBe("/agent/scout?panel=session:scout:run-2");
    expect(window.history.length).toBe(before);
  });

  it("opens and closes the workspace, keeping the open session", async () => {
    const { legacy, url } = await boot("/agent/scout?panel=session:scout:run-1");
    legacy.setWorkspace(true);
    expect(url()).toBe("/agent/scout/files?panel=session:scout:run-1");
    expect(legacy.view).toBe("workspace");
    legacy.setWorkspace(false);
    expect(url()).toBe("/agent/scout?panel=session:scout:run-1");
    expect(legacy.view).toBe("chat");
  });

  it("opens the team pages, the workbench, the scheduled view and the inbox", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openTeam("overview");
    expect(url()).toBe("/home");
    legacy.openTeam("files");
    expect(url()).toBe("/team/files");
    legacy.openWorkbench("tip-splitter");
    expect(url()).toBe("/team/workbench/tip-splitter");
    legacy.openWorkbench(null);
    expect(url()).toBe("/team/workbench");
    legacy.openScheduled();
    expect(url()).toBe("/agent/scout/schedule");
    legacy.openInbox();
    expect(url()).toBe("/inbox");
    expect(legacy.inbox).toBe(true);
  });

  it("returns to the agent's chat when a page is closed", async () => {
    const { legacy, url } = await boot("/agent/scout");
    legacy.openTeam("overview");
    legacy.closeTeam();
    expect(url()).toBe("/agent/scout");
    legacy.openWorkbench(null);
    legacy.closeWorkbench();
    expect(url()).toBe("/agent/scout");
    legacy.openScheduled();
    legacy.closeScheduled();
    expect(url()).toBe("/agent/scout");
    legacy.openInbox();
    legacy.closeInbox();
    expect(url()).toBe("/agent/scout");
  });

  it("keeps the workbench's full view out of the URL, and ends it with the artifact", async () => {
    const { legacy, url } = await boot("/team/workbench/tip-splitter");
    legacy.setWorkbenchFull(true);
    expect(url()).toBe("/team/workbench/tip-splitter");
    expect(legacy.workbench).toEqual({ artifact: "tip-splitter", full: true });
    legacy.openWorkbench("other");
    expect(legacy.workbench).toEqual({ artifact: "other", full: false });
    legacy.openWorkbench("tip-splitter");
    expect(legacy.workbench?.full).toBe(false);
    legacy.setWorkbenchFull(true);
    legacy.setWorkbenchFull(false);
    expect(legacy.workbench?.full).toBe(false);
  });

  it("opens no full view on the artifact list", async () => {
    const { legacy } = await boot("/team/workbench");
    legacy.setWorkbenchFull(true);
    expect(legacy.workbench).toEqual({ artifact: null, full: false });
  });

  it("does nothing that needs an agent when there is none", async () => {
    const { legacy, url } = await boot("/home");
    legacy.openScheduled();
    legacy.openSession("run-1");
    legacy.setWorkspace(true);
    expect(url()).toBe("/home");
    legacy.openMainChat();
    expect(url()).toBe("/home");
  });
});
