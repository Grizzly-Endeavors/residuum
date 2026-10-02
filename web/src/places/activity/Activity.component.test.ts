import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import type { OutboundA2aTaskSummary, SessionSummary } from "../../lib/types";
import { setViewedAgent } from "../../lib/viewed-agent";
import { ws } from "../../lib/ws.svelte";
import Activity from "./Activity.svelte";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    ...overrides,
  };
}

function run(runId: string, overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    address: `spawned-${runId}`,
    run_id: runId,
    category: "spawned",
    source_label: "agent:researcher",
    state: "running",
    spawner: "main",
    depth: 1,
    purpose: `Purpose of ${runId}`,
    started_at: "2026-03-14T11:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
    ...overrides,
  };
}

const LAPTOP: OutboundA2aTaskSummary = {
  task_id: "task-19c2",
  agent: "laptop",
  sender_address: "main",
  state: "working",
  status_text: null,
  open: true,
  started_at: "2026-03-14T11:00:00Z",
  unreachable_since: "2026-03-14T11:30:00Z",
};

let requests: string[];
let answer: (url: string, init: RequestInit | undefined) => Response;

beforeEach(() => {
  stubWebSocket();
  requests = [];
  answer = () => jsonResponse({ live: [], completed: [], next_cursor: null });
  mockFetch((url, init) => {
    requests.push(`${init?.method ?? "GET"} ${url}`);
    return answer(url, init);
  });
  hub.handleFrame(snapshot([agent("atlas"), agent("drifter", { state: "stopped" })]));
});

afterEach(() => {
  setViewedAgent(null);
});

/** atlas's Activity, with its sessions store holding `live`, `finished` and `outbound`. */
async function showAtlas(
  live: SessionSummary[],
  finished: SessionSummary[] = [],
  outbound: OutboundA2aTaskSummary[] = [],
): Promise<void> {
  setViewedAgent("atlas");
  ws.sessions.live = live;
  ws.sessions.outbound = outbound;
  ws.sessions.finished.all.mergeFirstPage(finished, null);
  ws.sessions.loaded = true;
  render(Activity, { agent: "atlas" });
  await settle();
}

describe("Activity", () => {
  it("lists what's running with its kind, and opens a run in the panel", async () => {
    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    await showAtlas([run("run-1"), run("run-2", { category: "scheduled", spawner: null })]);

    expect(screen.getByRole("heading", { name: /^Running now/ })).toHaveTextContent("2");
    expect(
      screen.getByRole("button", { name: /^Purpose of run-1 Started by atlas/ }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: /^Purpose of run-2 Scheduled/ }));
    expect(openPanel).toHaveBeenCalledWith({ kind: "session", agent: "atlas", runId: "run-2" });
  });

  it("stops a run from its row", async () => {
    answer = () => jsonResponse({ address: "spawned-run-1" }, 202);
    await showAtlas([run("run-1")]);
    await userEvent.click(screen.getByRole("button", { name: "Stop Purpose of run-1" }));
    expect(requests).toContain("POST /api/agents/atlas/sessions/spawned-run-1/stop");
  });

  it("says why a task can't be stopped, and stops watching it instead", async () => {
    answer = (url) =>
      url.endsWith("/stop")
        ? jsonResponse(
            { error: "Couldn't reach laptop to cancel the task.", code: "unreachable" },
            502,
          )
        : jsonResponse({ ...LAPTOP, open: false, state: "canceled" });
    await showAtlas([], [], [LAPTOP]);

    expect(screen.getByText("Sent to laptop")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Stop the task sent to laptop" }));
    expect(await screen.findByText("Couldn't reach laptop to cancel the task.")).toBeVisible();

    await userEvent.click(
      screen.getByRole("button", { name: "Stop watching the task sent to laptop" }),
    );
    await settle();
    expect(requests).toContain("POST /api/agents/atlas/a2a/outbound/task-19c2/stop-watching");
    expect(screen.queryByText("Sent to laptop")).toBeNull();
  });

  it("shows finished runs with how they ended, and filters them by kind", async () => {
    await showAtlas(
      [],
      [run("run-9", { state: "completed", outcome: "failed", error: "the site timed out" })],
    );
    expect(
      screen.getByText("Nothing running. Work atlas starts on its own shows up here."),
    ).toBeVisible();
    expect(screen.getByText("Failed: the site timed out")).toBeVisible();

    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "Kind of run" }),
      "artifact",
    );
    await settle();
    expect(requests).toContain("GET /api/agents/atlas/sessions?category=artifact&limit=25");
    expect(screen.getByText("Nothing of this kind has finished yet.")).toBeVisible();
  });

  it("shows a failed listing with Try again", async () => {
    setViewedAgent("atlas");
    ws.sessions.listError = "Couldn't load what's running. Residuum isn't reachable.";
    render(Activity, { agent: "atlas" });
    await settle();
    expect(screen.getByText(/^Couldn't load what's running\./)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(requests).toContain("GET /api/agents/atlas/sessions?limit=25");
  });

  it("offers Start for an agent that isn't running", async () => {
    setViewedAgent("drifter");
    render(Activity, { agent: "drifter" });
    await settle();
    expect(screen.getByRole("heading", { name: "drifter is stopped" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Start drifter" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: /^Running now/ })).toBeNull();
  });
});
