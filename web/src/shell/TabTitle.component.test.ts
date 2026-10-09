import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render, settle, stubWebSocket } from "../test/component";
import { activityFrame, snapshot } from "../test/hub-frames";
import { hub } from "../lib/hub.svelte";
import type { AgentSummary } from "../lib/hub-types";
import { router } from "../lib/router.svelte";
import { HOME } from "../lib/routes";
import TabTitle from "./TabTitle.svelte";

// The tab's title follows the router, the hub's activity and the page's
// visibility through effects, so this lives with the component tests.

function agent(name: string, displayName: string = name): AgentSummary {
  return {
    name,
    display_name: displayName,
    state: "running",
    last_error: null,
    autostart: false,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

function setVisibility(state: "visible" | "hidden"): void {
  Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));
}

let view: ReturnType<typeof render> | null = null;

beforeEach(async () => {
  stubWebSocket();
  setVisibility("visible");
  hub.handleFrame(snapshot([agent("atlas", "Atlas"), agent("scout")]));
  await router.replacePlace(HOME);
  view = render(TabTitle);
  await settle();
});

afterEach(async () => {
  view?.unmount();
  setVisibility("visible");
  await router.replacePlace(HOME);
});

describe("the tab's title", () => {
  it("is the app on Home, and the viewed agent's name on its places", async () => {
    expect(document.title).toBe("Residuum");

    await router.replacePlace({ kind: "chat", agent: "atlas" });
    await settle();
    expect(document.title).toBe("Atlas · Residuum");

    await router.replacePlace({ kind: "files", agent: "scout" });
    await settle();
    expect(document.title).toBe("scout · Residuum");

    await router.replacePlace({ kind: "shared-files" });
    await settle();
    expect(document.title).toBe("Shared files · Residuum");
  });

  it("goes back to the app's name when the shell goes", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    await settle();
    view?.unmount();
    view = null;
    expect(document.title).toBe("Residuum");
  });

  it("shows no markers while the tab is visible", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    hub.handleFrame(activityFrame("atlas", true, 3));
    await settle();
    expect(document.title).toBe("Atlas · Residuum");
  });

  it("counts what is unread, and says the agent is working, while the tab is hidden", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    hub.handleFrame(activityFrame("scout", false, 2));
    hub.handleFrame(activityFrame("atlas", true, 0));
    setVisibility("hidden");
    await settle();
    expect(document.title).toBe("(2) Atlas is working · Residuum");
  });

  it("says the agent finished when its turn ends under a hidden tab, until the tab is back", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    hub.handleFrame(activityFrame("atlas", true, 0));
    setVisibility("hidden");
    await settle();

    hub.handleFrame(activityFrame("atlas", false, 0));
    await settle();
    expect(document.title).toBe("Atlas finished · Residuum");

    setVisibility("visible");
    await settle();
    expect(document.title).toBe("Atlas · Residuum");
  });

  it("doesn't call a turn that ended before the tab was hidden finished", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    hub.handleFrame(activityFrame("atlas", true, 0));
    await settle();
    hub.handleFrame(activityFrame("atlas", false, 0));
    await settle();
    setVisibility("hidden");
    await settle();
    expect(document.title).toBe("Atlas · Residuum");
  });
});
