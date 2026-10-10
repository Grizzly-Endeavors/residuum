import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, mockFetch } from "../../test/component";
import { WorkbenchList } from "./workbench-list.svelte";
import { waitFor } from "../../test/wait";

// Through Residuum Cloud, opening an artifact first trades a handoff token for
// the workbench host's own credential; locally the link works as it is.

const RELAY = {
  ui_origin: "https://bear.agent-residuum.com",
  artifacts_origin: "https://bear.workbench.agent-residuum.com",
};

let tab: { opener: unknown; location: { href: string }; close: ReturnType<typeof vi.fn> };
let handoffAnswer: () => Response;
let info: unknown;

function list(pageOrigin: string): WorkbenchList {
  return new WorkbenchList({
    onFrame: () => () => {},
    page: () => ({
      origin: pageOrigin,
      protocol: pageOrigin.startsWith("https") ? "https:" : "http:",
      hostname: new URL(pageOrigin).hostname,
    }),
  });
}

function click(): { event: MouseEvent; prevented: () => boolean } {
  const event = new MouseEvent("click", { cancelable: true });
  return { event, prevented: () => event.defaultPrevented };
}

beforeEach(() => {
  tab = { opener: "page", location: { href: "" }, close: vi.fn() };
  handoffAnswer = () => jsonResponse({ token: "tok", expires_in_secs: 60 });
  info = { port: null, unavailable_reason: null, relay: RELAY };
  vi.stubGlobal("open", vi.fn().mockReturnValue(tab));
  mockFetch((url) => {
    if (url === "/api/team/workbench/artifacts") return jsonResponse([]);
    if (url === "/api/team/workbench/info") return jsonResponse(info);
    if (url === "/api/hub/devices/workbench-handoff") return handoffAnswer();
    return new Response("", { status: 404 });
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("opening an artifact through Residuum Cloud", () => {
  it("opens the handoff page, with the token and the artifact in the fragment", async () => {
    const bench = list(RELAY.ui_origin);
    await bench.load();
    const { event, prevented } = click();
    bench.open(event, "notes");
    expect(prevented()).toBe(true);
    await waitFor(() => {
      expect(tab.location.href).toBe(
        "https://bear.workbench.agent-residuum.com/_handoff#token=tok&next=%2Fnotes%2F",
      );
    });
    expect(tab.opener).toBeNull();
  });

  it("closes the tab it opened when the handoff can't be made", async () => {
    handoffAnswer = () => jsonResponse({ error: "no" }, 401);
    const bench = list(RELAY.ui_origin);
    await bench.load();
    bench.open(click().event, "notes");
    await waitFor(() => {
      expect(tab.close).toHaveBeenCalled();
    });
    expect(tab.location.href).toBe("");
  });

  it("leaves the link alone on a page that isn't served through Residuum Cloud", async () => {
    info = { port: 7702, unavailable_reason: null, relay: null };
    const bench = list("http://localhost:7700");
    await bench.load();
    const { event, prevented } = click();
    bench.open(event, "notes");
    expect(prevented()).toBe(false);
    expect(window.open).not.toHaveBeenCalled();
  });
});
