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
import { within } from "@testing-library/svelte";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { notifications } from "../../lib/notifications.svelte";
import { scheduled } from "../../lib/scheduled.svelte";
import type { ActionInfo, PulseInfo, ScheduledCurrentRun, ServerMessage } from "../../lib/types";
import { ws } from "../../lib/ws.svelte";
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
    model_tier: null,
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
  problem: null,
  current_run: null,
};

/** A fire of a pulse in flight, running its task scripts. */
function scriptRun(fireId: string, startedAt = soon(-1)): ScheduledCurrentRun {
  return {
    fire_id: fireId,
    phase: "script",
    started_at: startedAt,
    address: null,
    run_id: null,
    overlap: null,
  };
}

/** The list row of the pulse titled `title`, found by its switch. */
function rowOf(title: string): HTMLElement {
  const row = screen.getByRole("switch", { name: title }).closest("li");
  if (row === null) throw new Error(`no row for ${title}`);
  return row;
}

/** Take over the agent socket's frames, so a test can send the place one. */
function captureFrames(): { emit: (msg: ServerMessage) => void } {
  const listeners = new Set<(msg: ServerMessage, agent: string | null) => void>();
  vi.spyOn(ws, "onFrame").mockImplementation((listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  });
  return {
    emit: (msg) => {
      for (const listener of listeners) listener(msg, "atlas");
    },
  };
}

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
            fire_id: "fire-2",
            phase: "session",
            started_at: soon(-1),
            address: "scheduled-1",
            run_id: "run-2",
            overlap: {
              previous_run_id: "run-1",
              previous_fire_id: "fire-1",
              previous_started_at: soon(-12),
            },
          },
          last_outcome: { status: "failed", at: soon(-60), error: "the model timed out" },
        }),
        pulse("nightly_review", {
          enabled: false,
          next_fire_at: null,
          agent: "researcher",
          model_tier: "large",
        }),
        pulse("legacy_identity", {
          enabled: false,
          schedule: null,
          next_fire_at: null,
          problems: ["pulse 'legacy_identity' sets include_identity, which has been removed"],
        }),
      ],
      [
        digest,
        {
          ...digest,
          id: "act-2",
          name: "local_summary",
          run_at: soon(90),
          agent: null,
          model_tier: "lcoal/qwen3:8b",
          problem: "model 'lcoal/qwen3:8b' can't be used: 'lcoal' is not a known provider",
        },
      ],
    );
    render(Schedule, { agent: "atlas" });
    await screen.findByRole("heading", { name: "Pulses" });

    expect(screen.getByRole("heading", { name: "Pulses" })).toBeTruthy();
    expect(screen.getByText("Model: large tier")).toBeTruthy();
    expect(screen.getByText("Model: lcoal/qwen3:8b")).toBeTruthy();
    expect(
      screen.getByText("model 'lcoal/qwen3:8b' can't be used: 'lcoal' is not a known provider"),
    ).toBeTruthy();
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

  it("labels what each run in flight is doing, and links to its session once it has one", async () => {
    scheduled.reset("atlas");
    const session = {
      fire_id: "fire-2",
      phase: "session",
      started_at: soon(-5),
      address: "scheduled-2",
      run_id: "run-2",
      overlap: null,
    } as const;
    serveSchedule(
      [
        pulse("disk_watch", { current_run: scriptRun("fire-1", soon(-3)) }),
        pulse("inbox_check", { current_run: session }),
        pulse("nightly_review", { current_run: { ...session, fire_id: "fire-3", run_id: null } }),
      ],
      [],
    );
    render(Schedule, { agent: "atlas" });
    await screen.findByRole("heading", { name: "Pulses" });

    const disk = rowOf("Disk watch");
    expect(within(disk).getByText("Running scripts")).toBeTruthy();
    expect(within(disk).getByText("Started 3m ago")).toBeTruthy();
    expect(within(disk).queryByRole("link", { name: "Open session" })).toBeNull();
    expect(within(disk).getByRole("button", { name: "Stop Disk watch" })).toBeTruthy();

    const inbox = rowOf("Inbox check");
    expect(within(inbox).getByText("Running session")).toBeTruthy();
    expect(within(inbox).getByRole("link", { name: "Open session" })).toHaveAttribute(
      "href",
      "/agent/atlas/schedule?panel=session:atlas:run-2",
    );

    const nightly = rowOf("Nightly review");
    expect(within(nightly).getByText("Starting session")).toBeTruthy();
    expect(within(nightly).queryByRole("link", { name: "Open session" })).toBeNull();
  });

  it("shows how a pulse's last run without a session went, failures with their error", async () => {
    scheduled.reset("atlas");
    serveSchedule(
      [
        pulse("disk_watch", {
          last_outcome: {
            status: "failed",
            at: soon(-2),
            error: "check_disk.sh exited with status 1: /data is 97% full",
          },
        }),
      ],
      [],
    );
    render(Schedule, { agent: "atlas" });

    const line = await screen.findByText(
      "Last run failed 2m ago: check_disk.sh exited with status 1: /data is 97% full",
    );
    expect(line).toHaveAttribute("data-failed");
    expect(within(rowOf("Disk watch")).queryByRole("button", { name: /^Stop/ })).toBeNull();
  });

  it("stops a fire, shows it stopping, and shows the stopped run once it ends", async () => {
    const user = userEvent.setup();
    scheduled.reset("atlas");
    const frames = captureFrames();
    const pulses = [pulse("disk_watch", { current_run: scriptRun("fire-1") })];
    const requests = serveSchedule(pulses, []);
    render(Schedule, { agent: "atlas" });
    const stop = await screen.findByRole("button", { name: "Stop Disk watch" });

    await user.click(stop);
    expect(requests).toContain("/api/agents/atlas/scheduled/pulses/disk_watch/fires/fire-1/stop");
    expect(stop).toHaveAttribute("aria-busy", "true");

    // The fire ends as stopped, on the server and in the frame that says so.
    const stopped = { status: "cancelled", at: soon(0), error: null } as const;
    pulses[0] = pulse("disk_watch", { last_outcome: stopped });
    frames.emit({
      type: "pulse_fire_changed",
      pulse: "disk_watch",
      fire_id: "fire-1",
      current_run: null,
      last_outcome: stopped,
    });
    await settle();
    expect(screen.queryByRole("button", { name: "Stop Disk watch" })).toBeNull();
    expect(screen.queryByText("Running scripts")).toBeNull();
    expect(screen.getByText("Last run was stopped just now")).toBeTruthy();
  });

  it("stops an action's run through its session, and shows it stopping until it ends", async () => {
    const user = userEvent.setup();
    scheduled.reset("atlas");
    const frames = captureFrames();
    const run: ScheduledCurrentRun = {
      fire_id: null,
      phase: "session",
      started_at: soon(-2),
      address: "scheduled-3",
      run_id: "run-3",
      overlap: null,
    };
    const actions: ActionInfo[] = [{ ...digest, current_run: run }];
    const requests = serveSchedule([], actions);
    render(Schedule, { agent: "atlas" });
    const stop = await screen.findByRole("button", { name: "Stop Weekly digest" });

    const row = stop.closest("li");
    if (row === null) throw new Error("no row for Weekly digest");
    expect(within(row).getByText("Running session")).toBeTruthy();
    expect(within(row).getByRole("link", { name: "Open session" })).toHaveAttribute(
      "href",
      "/agent/atlas/schedule?panel=session:atlas:run-3",
    );

    await user.click(stop);
    expect(requests).toContain("/api/agents/atlas/sessions/scheduled-3/stop");
    expect(stop).toHaveAttribute("aria-busy", "true");

    // A load that still shows the run keeps Stop busy.
    await user.click(screen.getByRole("button", { name: "Reload the schedule" }));
    await settle();
    expect(screen.getByRole("button", { name: "Stop Weekly digest" })).toHaveAttribute(
      "aria-busy",
      "true",
    );

    // The session ends, and the load its frame starts no longer has the run.
    actions[0] = digest;
    frames.emit({
      type: "session_completed",
      address: "scheduled-3",
      run_id: "run-3",
      status: "cancelled",
      error: null,
      episode_id: null,
    });
    await settle();
    expect(screen.queryByRole("button", { name: "Stop Weekly digest" })).toBeNull();
    expect(screen.queryByText("Running session")).toBeNull();
    expect(scheduled.stopping.size).toBe(0);
  });

  it("stops the earlier fire a run overlaps, leaving the newer one running", async () => {
    const user = userEvent.setup();
    scheduled.reset("atlas");
    const overlapping = (previousFireId: string | null): ScheduledCurrentRun => ({
      ...scriptRun("fire-2"),
      overlap: {
        previous_run_id: null,
        previous_fire_id: previousFireId,
        previous_started_at: soon(-12),
      },
    });
    const requests = serveSchedule(
      [
        pulse("disk_watch", { current_run: overlapping("fire-1") }),
        // An earlier run no fire tracks has no fire to stop.
        pulse("inbox_check", { current_run: overlapping(null) }),
      ],
      [],
    );
    render(Schedule, { agent: "atlas" });
    const earlier = await screen.findByRole("button", {
      name: "Stop the earlier run of Disk watch",
    });
    expect(earlier.textContent.trim()).toBe("Stop earlier run");
    expect(within(rowOf("Inbox check")).queryByRole("button", { name: /earlier run/ })).toBeNull();

    await user.click(earlier);
    expect(requests).toContain("/api/agents/atlas/scheduled/pulses/disk_watch/fires/fire-1/stop");
    expect(requests.filter((url) => url.endsWith("/stop"))).toHaveLength(1);
    expect(earlier).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Stop Disk watch" })).not.toHaveAttribute(
      "aria-busy",
      "true",
    );
  });

  it("says when the fire had already finished, and lets Stop be pressed again", async () => {
    const user = userEvent.setup();
    scheduled.reset("atlas");
    const surface = vi.spyOn(notifications, "surface");
    const pulses = [pulse("disk_watch", { current_run: scriptRun("fire-1") })];
    mockFetch((url) => {
      if (url.endsWith("/stop")) {
        return jsonResponse(
          { error: 'This run of the pulse "disk_watch" isn\'t running any more.' },
          404,
        );
      }
      if (url.endsWith("/scheduled/pulses")) return jsonResponse(pulses);
      return jsonResponse([]);
    });
    render(Schedule, { agent: "atlas" });

    await user.click(await screen.findByRole("button", { name: "Stop Disk watch" }));
    await settle();
    expect(surface).toHaveBeenCalledWith(
      "error",
      'Couldn\'t stop pulse "disk_watch". It had already finished.',
    );
    expect(screen.getByRole("button", { name: "Stop Disk watch" })).not.toHaveAttribute(
      "aria-busy",
      "true",
    );
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
