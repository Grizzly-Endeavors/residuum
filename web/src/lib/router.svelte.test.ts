import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as PathsModule from "./paths";
import type { router as routerInstance } from "./router.svelte";

/** A stand-in for the browser's location and history, recording every navigation. */
interface FakeBrowser {
  url: () => string;
  pushes: string[];
  replaces: string[];
  /** Simulate the back or forward button landing on `url`. */
  pop: (url: string) => void;
}

function installBrowser(startUrl: string, lastAgent: string | null): FakeBrowser {
  const [startPath = "/", startSearch = ""] = startUrl.split("?");
  const location = { pathname: startPath, search: startSearch === "" ? "" : `?${startSearch}` };
  const pushes: string[] = [];
  const replaces: string[] = [];
  const listeners: (() => void)[] = [];

  function set(url: string): void {
    const [path = "/", search = ""] = url.split("?");
    location.pathname = path;
    location.search = search === "" ? "" : `?${search}`;
  }

  vi.stubGlobal("window", {
    location,
    history: {
      pushState: (_state: unknown, _title: string, url: string) => {
        pushes.push(url);
        set(url);
      },
      replaceState: (_state: unknown, _title: string, url: string) => {
        replaces.push(url);
        set(url);
      },
    },
    addEventListener: (_type: string, listener: () => void) => {
      listeners.push(listener);
    },
  });
  const storage = new Map<string, string>();
  if (lastAgent !== null) storage.set("residuum-last-agent", lastAgent);
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => {
      storage.set(key, value);
    },
  });

  return {
    url: () => `${location.pathname}${location.search}`,
    pushes,
    replaces,
    pop: (url) => {
      set(url);
      for (const listener of listeners) listener();
    },
  };
}

/** A fresh router and path module, so no test inherits another's location. */
async function load(): Promise<{ router: typeof routerInstance; paths: typeof PathsModule }> {
  vi.resetModules();
  const { router } = await import("./router.svelte");
  const paths = await import("./paths");
  return { router, paths };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("router: opening the app", () => {
  it("sends / to the last-used agent", async () => {
    const browser = installBrowser("/", "atlas");
    const { router, paths } = await load();
    router.start();
    expect(browser.url()).toBe("/agent/atlas");
    expect(router.agent).toBe("atlas");
    expect(paths.getCurrentAgent()).toBe("atlas");
  });

  it("leaves / alone with no last-used agent, until the agents are known", async () => {
    const browser = installBrowser("/", null);
    const { router, paths } = await load();
    router.start();
    expect(browser.url()).toBe("/");
    expect(router.agent).toBeNull();
    expect(paths.getCurrentAgent()).toBeNull();

    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/agent/atlas");
    expect(router.agent).toBe("atlas");
    expect(paths.getCurrentAgent()).toBe("atlas");
  });

  it("does nothing on / when there are no agents at all", async () => {
    const browser = installBrowser("/", null);
    const { router } = await load();
    router.start();
    router.resolveAgent([]);
    expect(browser.url()).toBe("/");
    expect(router.agent).toBeNull();
  });

  it("prefers the last-used agent over the first when it still exists", async () => {
    const browser = installBrowser("/", "scout");
    const { router } = await load();
    router.start();
    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/agent/scout");
  });

  it("falls back to the first agent when the last-used one is gone", async () => {
    const browser = installBrowser("/", "gone");
    const { router } = await load();
    router.start();
    expect(browser.url()).toBe("/agent/gone");
    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/agent/atlas");
    expect(router.agent).toBe("atlas");
  });

  it("moves off an agent in the URL that doesn't exist", async () => {
    const browser = installBrowser("/agent/ghost/sessions/run-1", "scout");
    const { router } = await load();
    router.start();
    expect(router.agent).toBe("ghost");
    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/agent/scout");
    expect(router.chat.runId).toBeNull();
  });

  it("keeps an agent that exists, and the page the user is on", async () => {
    const browser = installBrowser("/agent/scout/sessions/run-1", null);
    const { router } = await load();
    router.start();
    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/agent/scout/sessions/run-1");
    expect(browser.replaces).toEqual([]);
  });

  it("gives a team page an agent without leaving the page", async () => {
    const browser = installBrowser("/team/files", null);
    const { router } = await load();
    router.start();
    expect(router.agent).toBeNull();
    router.resolveAgent(["atlas", "scout"]);
    expect(browser.url()).toBe("/team/files");
    expect(router.agent).toBe("atlas");
    expect(router.team).toBe("files");
  });

  it("uses the last-used agent on a team page straight away", async () => {
    installBrowser("/team", "scout");
    const { router, paths } = await load();
    router.start();
    expect(router.agent).toBe("scout");
    expect(paths.getCurrentAgent()).toBe("scout");
  });

  it("rewrites an older unprefixed path under the agent", async () => {
    const browser = installBrowser("/settings/memory", "scout");
    const { router } = await load();
    router.start();
    expect(browser.url()).toBe("/agent/scout/settings/memory");
    expect(router.settings).toEqual({ scope: "agent", section: "memory" });
  });

  it("remembers an agent once it is known to exist, not before", async () => {
    installBrowser("/agent/atlas", "scout");
    const { router, paths } = await load();
    router.start();
    expect(paths.readLastAgent()).toBe("scout");
    router.resolveAgent(["atlas", "scout"]);
    expect(paths.readLastAgent()).toBe("atlas");
  });

  it("remembers the agent the user switches to", async () => {
    installBrowser("/agent/scout", null);
    const { router, paths } = await load();
    router.start();
    router.openAgent("atlas");
    expect(paths.readLastAgent()).toBe("atlas");
  });
});

describe("router: switching agents", () => {
  let browser: FakeBrowser;

  beforeEach(() => {
    browser = installBrowser("/agent/scout", null);
  });

  it("changes the URL and the current agent", async () => {
    const { router, paths } = await load();
    router.start();
    router.openAgent("atlas");
    expect(browser.pushes).toEqual(["/agent/atlas"]);
    expect(router.agent).toBe("atlas");
    expect(paths.getCurrentAgent()).toBe("atlas");
  });

  it("does nothing when the agent is already open", async () => {
    const { router } = await load();
    router.start();
    router.openAgent("scout");
    expect(browser.pushes).toEqual([]);
  });

  it("leaves the previous agent's open session behind", async () => {
    const { router } = await load();
    router.start();
    router.openSession("run-1");
    router.openAgent("atlas");
    expect(browser.url()).toBe("/agent/atlas");
    expect(router.chat.runId).toBeNull();
  });

  it("keeps the same page when it exists for every agent", async () => {
    const { router } = await load();
    router.start();
    router.openSettings("memory");
    router.openAgent("atlas");
    expect(browser.url()).toBe("/agent/atlas/settings/memory");

    router.openScheduled();
    router.openAgent("scout");
    expect(browser.url()).toBe("/agent/scout/scheduled");
  });

  it("goes to the agent's chat from a team page", async () => {
    const { router } = await load();
    router.start();
    router.openTeam("overview");
    expect(browser.url()).toBe("/team");
    router.openAgent("atlas");
    expect(browser.url()).toBe("/agent/atlas");
  });

  it("follows the back button to the previous agent", async () => {
    const { router, paths } = await load();
    router.start();
    router.openAgent("atlas");
    browser.pop("/agent/scout");
    expect(router.agent).toBe("scout");
    expect(paths.getCurrentAgent()).toBe("scout");
  });

  it("keeps the agent while moving around the team pages", async () => {
    const { router, paths } = await load();
    router.start();
    router.openTeam("files");
    router.openWorkbench("chart");
    expect(browser.url()).toBe("/team/workbench/chart");
    expect(router.agent).toBe("scout");
    router.openSettings("a2a", "hub");
    expect(browser.url()).toBe("/team/settings/a2a");
    expect(paths.getCurrentAgent()).toBe("scout");
    router.closeSettings();
    expect(browser.url()).toBe("/agent/scout");
  });
});
