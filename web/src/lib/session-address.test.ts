// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { notifications } from "./notifications.svelte";
import { router } from "./router.svelte";
import { HOME } from "./routes";
import { openSessionByAddress } from "./session-address";

function respond(body: unknown, status = 200): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn((_url: string) =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

const NEWEST = { live: [{ run_id: "run-7" }], completed: [], next_cursor: null };

afterEach(async () => {
  await router.replacePlace(HOME);
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("opening a session from where it is mentioned", () => {
  it("opens the run the caller names over its own agent's place", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    const openPanel = vi.spyOn(router, "openPanel");
    await openSessionByAddress("atlas", "spawned-x-1", "run-3");
    expect(openPanel).toHaveBeenCalledWith({ kind: "session", agent: "atlas", runId: "run-3" });
  });

  // No agent is bound here, so scout's sessions aren't loaded: the run is asked of scout.
  it("looks up another agent's newest run on that agent, and opens it beside its chat", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    const fetchMock = respond(NEWEST);
    const openPlace = vi.spyOn(router, "openPlace");

    await openSessionByAddress("scout", "spawned-y-2", null);

    expect(fetchMock.mock.calls[0]?.[0]).toContain("/api/agents/scout/sessions");
    expect(fetchMock.mock.calls[0]?.[0]).toContain("address=spawned-y-2");
    expect(openPlace).toHaveBeenCalledWith(
      { kind: "chat", agent: "scout" },
      { panel: { kind: "session", agent: "scout", runId: "run-7" } },
    );
  });

  it("opens a run on any agent over the Workbench, where the panel shows every agent's runs", async () => {
    await router.replacePlace({ kind: "workbench", artifact: null });
    const openPanel = vi.spyOn(router, "openPanel");
    await openSessionByAddress("scout", "artifact-tip-1", "run-1");
    expect(openPanel).toHaveBeenCalledWith({ kind: "session", agent: "scout", runId: "run-1" });
  });

  it("tells the user when there is no such session, and stays put", async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
    respond({ live: [], completed: [], next_cursor: null });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    const openPanel = vi.spyOn(router, "openPanel");

    await openSessionByAddress("scout", "spawned-gone-1", null);

    expect(surface).toHaveBeenCalledWith(
      "error",
      "There's no record of the session spawned-gone-1.",
    );
    expect(openPanel).not.toHaveBeenCalled();
  });
});
