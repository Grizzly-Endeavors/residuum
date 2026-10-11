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
import { scheduled } from "../../lib/scheduled.svelte";
import type { ActionInfo, PulseInfo } from "../../lib/types";
import Schedule from "./Schedule.svelte";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
    ...overrides,
  };
}

const soon = (minutes: number): string => new Date(Date.now() + minutes * 60_000).toISOString();

function pulse(name: string, overrides: Partial<PulseInfo> = {}): PulseInfo {
  return {
    name,
    enabled: true,
    schedule: "2h",
    active_hours: null,
    agent: null,
    next_fire_at: soon(34),
    last_outcome: null,
    current_run: null,
    problems: [],
    ...overrides,
  };
}

const digest: ActionInfo = {
  id: "act-1",
  name: "weekly_digest",
  run_at: soon(20),
  agent: "researcher",
  model_tier: null,
  current_run: null,
};

/** Answer the schedule routes with `pulses` and `actions`; `fail` answers them with a fault. */
function serveSchedule(
  pulses: PulseInfo[],
  actions: ActionInfo[],
  state: { fail: boolean } = { fail: false },
): string[] {
  const requests: string[] = [];
  mockFetch((url) => {
    requests.push(url);
    if (state.fail) return jsonResponse({ error: "boom" }, 500);
    if (url.endsWith("/scheduled/pulses")) return jsonResponse(pulses);
    if (url.endsWith("/scheduled/actions")) return jsonResponse(actions);
    return jsonResponse({});
  });
  return requests;
}

beforeEach(() => {
  stubWebSocket();
  hub.handleFrame(
    snapshot([
      agent("atlas"),
      agent("drifter", { state: "stopped" }),
      agent("brittle", { state: "failed", last_error: null }),
    ]),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Schedule", () => {
  it("lists pulses and actions with their state, next run and controls", async () => {
    scheduled.reset("atlas");
    serveSchedule(
      [
        pulse("inbox_check", {
          active_hours: "08:00-22:00",
          current_run: {
            address: "scheduled-1",
            run_id: "run-2",
            overlap: { previous_run_id: "run-1", previous_started_at: soon(-12) },
          },
          last_outcome: { status: "failed", at: soon(-60), error: "the model timed out" },
        }),
        pulse("nightly_review", { enabled: false, next_fire_at: null, agent: "researcher" }),
        pulse("legacy_identity", {
          enabled: false,
          schedule: null,
          next_fire_at: null,
          problems: ["pulse 'legacy_identity' sets include_identity, which has been removed"],
        }),
      ],
      [digest],
    );
    render(Schedule, { agent: "atlas" });
    await screen.findByRole("heading", { name: "Pulses" });

    expect(screen.getByRole("heading", { name: "Pulses" })).toBeTruthy();
    expect(screen.getByText("Every 2 hours, between 08:00 and 22:00")).toBeTruthy();
    expect(screen.getByText("Started while the run from 12m ago was still going")).toBeTruthy();
    expect(screen.getByText("Last run failed 1h ago: the model timed out")).toBeTruthy();
    expect(screen.getByText("in 34m")).toBeTruthy();
    expect(screen.getByText("Paused")).toBeTruthy();
    expect(screen.getByText("Won't run until fixed")).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Inbox check" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("switch", { name: "Nightly review" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    expect(screen.getByRole("switch", { name: "Legacy identity" })).toBeDisabled();

    expect(screen.getByText("Weekly digest")).toBeTruthy();
    expect(screen.getByText("in 20m")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel Weekly digest" })).toBeTruthy();
  });

  it("shows a failed load with Try again, never the empty states", async () => {
    const user = userEvent.setup();
    scheduled.reset("atlas");
    const server = { fail: true };
    serveSchedule([], [], server);
    render(Schedule, { agent: "atlas" });
    await screen.findByRole("alert");

    expect(screen.getByRole("alert").textContent).toMatch(/Couldn't load the schedule\./);
    expect(screen.queryByText(/No pulses yet/)).toBeNull();
    expect(screen.queryByText(/Nothing scheduled/)).toBeNull();

    server.fail = false;
    await user.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByText(/No pulses yet/);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText(/No pulses yet/)).toBeTruthy();
    expect(screen.getByText(/Nothing scheduled/)).toBeTruthy();
  });

  it("says when no pulse that should run will, and offers the settings", async () => {
    scheduled.reset("atlas");
    serveSchedule([pulse("inbox_check", { next_fire_at: null })], []);
    render(Schedule, { agent: "atlas" });
    await screen.findByRole("heading", { name: "Pulses" });

    expect(screen.getByText(/None of these pulses will run/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open settings" })).toBeTruthy();
    expect(screen.getByText("No next run")).toBeTruthy();
  });

  it("offers Start for a stopped agent instead of loading its schedule", async () => {
    const user = userEvent.setup();
    scheduled.reset("drifter");
    const requests = serveSchedule([], []);
    const start = vi.spyOn(hub, "startAgent").mockResolvedValue(true);
    render(Schedule, { agent: "drifter" });
    await settle();

    expect(screen.getByRole("heading", { name: "drifter is stopped" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(requests.filter((url) => url.includes("/scheduled/"))).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Start drifter" }));
    expect(start).toHaveBeenCalledWith("drifter");
  });

  it("says why a failed agent can't start", async () => {
    scheduled.reset("brittle");
    serveSchedule([], []);
    render(Schedule, { agent: "brittle" });
    await settle();

    expect(screen.getByRole("heading", { name: "brittle can't start" })).toBeTruthy();
    expect(screen.getByText(/Something went wrong while it was starting\./)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Start brittle" })).toBeTruthy();
  });
});
