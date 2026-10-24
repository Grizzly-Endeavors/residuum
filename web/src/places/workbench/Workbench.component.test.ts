import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import {
  advance,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { hub } from "../../lib/hub.svelte";
import type { AgentOverview, LiveSession } from "../../lib/hub-types";
import { overview } from "../../lib/overview.svelte";
import { router } from "../../lib/router.svelte";
import { toast } from "../../lib/toast.svelte";
import type { ArtifactSummary, WorkbenchInfo } from "../../lib/types";
import Workbench from "./Workbench.svelte";
import { CHANGE_GLOW_MS } from "./workbench-list.svelte";
import { waitFor } from "../../test/wait";

const tip: ArtifactSummary = {
  name: "tip-splitter",
  title: "Tip Splitter",
  modified_at: new Date(Date.now() - 5 * 60_000).toISOString(),
  size: 1200,
};
const graph: ArtifactSummary = {
  name: "wiki-graph",
  title: "Wiki graph",
  modified_at: new Date(Date.now() - 60 * 60_000).toISOString(),
  size: 900,
};
const serving: WorkbenchInfo = { port: 7702, unavailable_reason: null, relay: null };

function live(agent: string, runId: string, purpose: string): LiveSession {
  return {
    address: `artifact-wiki-graph-${agent}`,
    run_id: runId,
    category: "artifact",
    source_label: "artifact:wiki-graph",
    purpose,
    state: "running",
    started_at: new Date(Date.now() - 2 * 60_000).toISOString(),
  };
}

function overviewOf(name: string, sessions: LiveSession[]): AgentOverview {
  return {
    name,
    last_message: null,
    live_sessions: sessions,
    upcoming: [],
    inbox_unread: 0,
    outbound_problems: [],
  };
}

interface Bench {
  artifacts: ArtifactSummary[];
  info: WorkbenchInfo;
  fail: boolean;
  requests: string[];
}

/** Answer the workbench routes from `bench`, which a test may change between reads. */
function serveBench(initial: Partial<Bench> = {}): Bench {
  const bench: Bench = {
    artifacts: [tip, graph],
    info: serving,
    fail: false,
    requests: [],
    ...initial,
  };
  mockFetch((url, init) => {
    bench.requests.push(`${init?.method ?? "GET"} ${url}`);
    if (bench.fail) return jsonResponse({ error: "boom" }, 500);
    if (url.endsWith("/workbench/artifacts")) return jsonResponse(bench.artifacts);
    if (url.endsWith("/workbench/info")) return jsonResponse(bench.info);
    if (url.includes("/workbench/artifacts/") && init?.method === "DELETE") {
      return jsonResponse({ removed: ["tip-splitter.html"], checkpoint_id: "cp-1" });
    }
    return jsonResponse({});
  });
  return bench;
}

const reads = (bench: Bench): number =>
  bench.requests.filter((r) => r.endsWith("/workbench/artifacts")).length;

beforeEach(() => {
  stubWebSocket();
  // jsdom has no layout, so nothing scrolls; a selected row asks to be shown.
  Element.prototype.scrollIntoView = vi.fn();
  overview.overviews = {};
  vi.spyOn(router, "resolveArtifacts").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  toast.toasts.clear();
});

describe("Workbench", () => {
  it("lists each page with its path and last edit, and opens it in a new tab on the artifacts origin", async () => {
    serveBench();
    render(Workbench, { artifact: null });
    await screen.findByText("Tip Splitter");

    expect(screen.getByText("/team/workbench/tip-splitter")).toBeTruthy();
    expect(screen.getByText("edited 5m ago")).toBeTruthy();
    const open = screen.getByRole("link", { name: "Open Tip Splitter" });
    expect(open).toHaveAttribute("href", "http://localhost:7702/tip-splitter/");
    expect(open).toHaveAttribute("target", "_blank");
    expect(open).toHaveAttribute("rel", "noopener");
  });

  it("says why pages can't open, even with nothing on the bench, and has no address to open", async () => {
    serveBench({
      artifacts: [],
      info: { port: null, unavailable_reason: "Port 7702 is taken.", relay: null },
    });
    render(Workbench, { artifact: null });

    expect(await screen.findByText("Workbench pages can't open right now.")).toBeTruthy();
    expect(screen.getByText("Port 7702 is taken.")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Nothing on the bench yet" })).toBeTruthy();
  });

  it("disables Open while pages can't open, and reads again until they can", async () => {
    vi.useFakeTimers();
    const bench = serveBench({
      info: { port: null, unavailable_reason: "Port 7702 is taken.", relay: null },
    });
    render(Workbench, { artifact: null });
    await settle();
    expect(screen.getByRole("button", { name: "Open Tip Splitter" })).toBeDisabled();

    bench.info = serving;
    await advance(10_000);
    expect(screen.queryByText("Workbench pages can't open right now.")).toBeNull();
    expect(screen.getByRole("link", { name: "Open Tip Splitter" })).toBeTruthy();
  });

  it("shows a failed read with Try again, which reads again", async () => {
    const user = userEvent.setup();
    const bench = serveBench({ fail: true });
    render(Workbench, { artifact: null });

    expect(await screen.findByText(/^Couldn't load the workbench\./)).toBeTruthy();
    expect(screen.queryByText("Nothing on the bench yet")).toBeNull();
    bench.fail = false;
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("Tip Splitter")).toBeTruthy();
  });

  it("marks a page an agent changes as updating now, reads the list again, and settles", async () => {
    vi.useFakeTimers();
    const bench = serveBench();
    render(Workbench, { artifact: null });
    await settle();
    const before = reads(bench);

    hub.handleFrame({ type: "artifact_updated", name: "tip-splitter" });
    await settle();
    expect(screen.getByText("updating now")).toBeTruthy();
    expect(reads(bench)).toBe(before + 1);

    await advance(CHANGE_GLOW_MS);
    expect(screen.queryByText("updating now")).toBeNull();
    expect(screen.getByText("edited 5m ago")).toBeTruthy();
  });

  it("reads the list again when the hub reconnects, and settles the URL on the fresh list", async () => {
    const resolve = vi.spyOn(router, "resolveArtifacts").mockImplementation(() => undefined);
    const bench = serveBench();
    render(Workbench, { artifact: "wiki-graph" });
    await screen.findByText("Tip Splitter");
    expect(resolve).toHaveBeenLastCalledWith(["tip-splitter", "wiki-graph"]);

    bench.artifacts = [tip];
    hub.handleFrame({ type: "hub_boot", boot_id: "boot-2" });
    await settle();
    await waitFor(() => {
      expect(resolve).toHaveBeenLastCalledWith(["tip-splitter"]);
    });
  });

  it("counts a page's running sessions on every agent, and the selected row lists them", async () => {
    const user = userEvent.setup();
    serveBench();
    overview.overviews = {
      atlas: overviewOf("atlas", [live("atlas", "run-a", "Link orphaned pages")]),
      scout: overviewOf("scout", [live("scout", "run-s", "Check the graph's links")]),
    };
    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    render(Workbench, { artifact: "wiki-graph" });
    await screen.findByText("Wiki graph");

    expect(screen.getByText("2 sessions running")).toBeTruthy();
    expect(screen.getByText("http://localhost:7702/wiki-graph/")).toBeTruthy();
    expect(screen.getByText("On atlas")).toBeTruthy();
    expect(screen.getByText("On scout")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: /^Check the graph's links/ }));
    expect(openPanel).toHaveBeenCalledWith({ kind: "session", agent: "scout", runId: "run-s" });
  });

  it("stops one of a page's sessions on its own agent", async () => {
    const user = userEvent.setup();
    const bench = serveBench();
    overview.overviews = {
      atlas: overviewOf("atlas", [live("atlas", "run-a", "Link orphaned pages")]),
    };
    render(Workbench, { artifact: "wiki-graph" });
    await screen.findByText("Wiki graph");

    await user.click(screen.getByRole("button", { name: "Stop Link orphaned pages on atlas" }));
    expect(bench.requests).toContain(
      "POST /api/agents/atlas/sessions/artifact-wiki-graph-atlas/stop",
    );
  });

  it("says a selected page with nothing running has nothing running", async () => {
    serveBench();
    render(Workbench, { artifact: "tip-splitter" });
    expect(
      await screen.findByText(
        "Nothing running. Sessions this page starts show here, on any agent.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { expanded: true })).toHaveTextContent(/^Tip Splitter/);
  });

  it("deletes a page at once and offers Undo on the toast", async () => {
    const user = userEvent.setup();
    const bench = serveBench();
    render(Workbench, { artifact: null });
    await screen.findByText("Tip Splitter");

    await user.click(screen.getByRole("button", { name: "More for Tip Splitter" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await waitFor(() => {
      expect(screen.queryByText("Tip Splitter")).toBeNull();
    });
    expect(bench.requests).toContain("DELETE /api/team/workbench/artifacts/tip-splitter");
    const [shown] = [...toast.toasts.values()];
    expect(shown?.message).toBe("Deleted “Tip Splitter”.");
    expect(shown?.action?.label).toBe("Undo");
  });

  it("copies a page's link, and gives the link to copy by hand when the browser won't", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    serveBench();
    render(Workbench, { artifact: "tip-splitter" });
    await screen.findByText("Tip Splitter");

    await user.click(screen.getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith("http://localhost:7702/tip-splitter/");

    writeText.mockImplementation(() => Promise.reject(new Error("not allowed")));
    await user.click(screen.getByRole("button", { name: "Copy link" }));
    await waitFor(() => {
      const messages = [...toast.toasts.values()].map((t) => t.message);
      expect(messages).toContain(
        "Couldn't copy the link. Here it is to copy by hand: http://localhost:7702/tip-splitter/",
      );
    });
  });
});
