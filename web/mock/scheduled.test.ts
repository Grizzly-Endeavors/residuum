import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  ActionInfo,
  PulseInfo,
  ServerMessage,
  SessionSummary,
} from "../src/lib/generated/protocol";
import { untrackedRunFields } from "./data/sessions";
import { createMockEnv } from "./env";
import { scheduledRoutes } from "./scheduled";
import { advanceMockTime, fetchJson, startRouteHarness, type RouteHarness } from "./test-support";

describe("the Scheduled view routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(scheduledRoutes, createMockEnv({ deterministic: true }));
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/scheduled${path}`;
  const pulses = async (): Promise<PulseInfo[]> =>
    (await fetchJson(url("/pulses"))).body as PulseInfo[];
  const put = (path: string, body: unknown): Promise<{ status: number; body: unknown }> =>
    fetchJson(url(path), {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });

  /** A scheduled run of `source` that is live now. */
  function startRun(source: string): SessionSummary {
    const run: SessionSummary = {
      address: "scheduled-run-1",
      run_id: "run-sched-1",
      category: "scheduled",
      source_label: source,
      state: "running",
      spawner: null,
      depth: 1,
      purpose: "Run it",
      started_at: harness.state.env.clock.iso(),
      completed_at: null,
      episode_id: null,
      interrupted: false,
      ...untrackedRunFields(),
    };
    harness.state.sessions.live.unshift(run);
    return run;
  }

  describe("pulses", () => {
    it("lists each pulse with its schedule, next fire and last outcome", async () => {
      const list = await pulses();
      expect(list.map((p) => p.name)).toEqual(["inbox_check", "nightly_review", "legacy_identity"]);
      const check = list[0];
      expect(check).toMatchObject({
        enabled: true,
        schedule: "2h",
        active_hours: "08:00-22:00",
        problems: [],
        current_run: null,
      });
      // The last run was an hour ago and the schedule is two hours.
      expect(check?.next_fire_at).toBe(harness.state.env.clock.isoIn(3_600_000));
      // The newest finished `pulse:inbox_check` run in the sample sessions.
      expect(check?.last_outcome).toMatchObject({ status: "completed", error: null });
    });

    it("gives a disabled pulse no next fire, and a pulse that failed to load its problems", async () => {
      const [, disabled, rejected] = await pulses();
      expect(disabled).toMatchObject({ enabled: false, next_fire_at: null });
      expect(rejected).toMatchObject({
        enabled: false,
        schedule: null,
        next_fire_at: null,
        problems: ["pulse 'legacy_identity' sets include_identity, which has been removed"],
      });
    });

    it("shows a live run of the pulse as its current run", async () => {
      const run = startRun("pulse:inbox_check");
      const [check] = await pulses();
      expect(check?.current_run).toEqual({
        fire_id: null,
        phase: "session",
        started_at: run.started_at,
        address: run.address,
        run_id: run.run_id,
        overlap: null,
      });
    });

    it("toggles a pulse and answers with its name and new state", async () => {
      expect(await put("/pulses/nightly_review/enabled", { enabled: true })).toEqual({
        status: 200,
        body: { name: "nightly_review", enabled: true },
      });
      const [, nightly] = await pulses();
      expect(nightly?.enabled).toBe(true);
      // Enabled with a last run 20 hours ago on a daily schedule.
      expect(nightly?.next_fire_at).toBe(harness.state.env.clock.isoIn(4 * 3_600_000));
    });

    it("answers 404 for a pulse HEARTBEAT.yml doesn't have, and 400 for a body without enabled", async () => {
      expect(await put("/pulses/ghost/enabled", { enabled: true })).toEqual({
        status: 404,
        body: { error: 'No pulse named "ghost" was found in HEARTBEAT.yml.' },
      });
      const refused = await put("/pulses/inbox_check/enabled", { enabled: "yes" });
      expect(refused.status).toBe(400);
      expect((refused.body as { error: string }).error).toContain("invalid request body");
    });
  });

  describe("pulse fires", () => {
    const post = (path: string, body?: unknown): Promise<{ status: number; body: unknown }> =>
      fetchJson(`${harness.baseUrl}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });

    async function fire(pulse = "inbox_check"): Promise<string> {
      const { status, body } = await post("/api/mock/pulse-fire", { pulse });
      expect(status, JSON.stringify(body)).toBe(200);
      return (body as { fire_id: string }).fire_id;
    }

    const fireFrames = (): ServerMessage[] =>
      harness.frames.filter((frame) => frame.type === "pulse_fire_changed");

    it("shows a fire in its scripts as the pulse's current run, and announces it", async () => {
      const fireId = await fire();
      const startedAt = harness.state.env.clock.iso();
      const [check] = await pulses();
      const run = {
        fire_id: fireId,
        phase: "script",
        started_at: startedAt,
        address: null,
        run_id: null,
        overlap: null,
      };
      expect(check?.current_run).toEqual(run);
      // The pulse ran now, so its next run is one schedule away.
      expect(check?.next_fire_at).toBe(harness.state.env.clock.isoIn(2 * 3_600_000));
      expect(fireFrames()).toEqual([
        {
          type: "pulse_fire_changed",
          pulse: "inbox_check",
          fire_id: fireId,
          current_run: run,
          last_outcome: null,
        },
      ]);
    });

    it("points a second fire's overlap at the first, still in its scripts", async () => {
      const first = await fire();
      const second = await fire();
      const [check] = await pulses();
      expect(check?.current_run).toMatchObject({
        fire_id: second,
        overlap: {
          previous_fire_id: first,
          previous_run_id: null,
          previous_started_at: harness.state.env.clock.iso(),
        },
      });
    });

    it("makes a fire's outcome the pulse's last outcome, newer than its last session", async () => {
      const fireId = await fire();
      const finish = await post("/api/mock/pulse-fire/finish", {
        fire_id: fireId,
        status: "failed",
        error: "check_inbox.sh exited with status 2",
      });
      expect(finish.status).toBe(200);
      const outcome = {
        status: "failed",
        at: harness.state.env.clock.iso(),
        error: "check_inbox.sh exited with status 2",
      };
      const [check] = await pulses();
      expect(check).toMatchObject({ current_run: null, last_outcome: outcome });
      expect(fireFrames().at(-1)).toMatchObject({ current_run: null, last_outcome: outcome });
    });

    it("stops a fire a moment after answering, and answers 404 once it has gone", async () => {
      // Simulated time moves only when the test moves it, so the stop waits.
      harness.state.env.setTimeMode("manual");
      const fireId = await fire();
      const stop = (): Promise<{ status: number; body: unknown }> =>
        post(`/api/scheduled/pulses/inbox_check/fires/${fireId}/stop`);
      expect(await stop()).toEqual({
        status: 202,
        body: { pulse: "inbox_check", fire_id: fireId },
      });
      // Asked again while it finishes, it answers the same.
      expect((await stop()).status).toBe(202);
      expect((await pulses())[0]?.current_run).toMatchObject({ fire_id: fireId });

      await advanceMockTime(harness.state.env, 300);
      const [check] = await pulses();
      expect(check).toMatchObject({
        current_run: null,
        last_outcome: { status: "cancelled", error: null },
      });
      expect(fireFrames()).toMatchObject([
        { last_outcome: null },
        { current_run: null, last_outcome: { status: "cancelled" } },
      ]);
      expect(await stop()).toEqual({
        status: 404,
        body: { error: 'This run of the pulse "inbox_check" isn\'t running any more.' },
      });
    });

    it("answers 404 for a fire of another pulse", async () => {
      const fireId = await fire();
      const { status } = await post(`/api/scheduled/pulses/nightly_review/fires/${fireId}/stop`);
      expect(status).toBe(404);
      expect((await pulses())[0]?.current_run).toMatchObject({ fire_id: fireId });
    });

    it("refuses a control for a pulse, fire or status it doesn't know", async () => {
      expect((await post("/api/mock/pulse-fire", { pulse: "ghost" })).status).toBe(404);
      expect(
        (await post("/api/mock/pulse-fire/finish", { fire_id: "fire-x", status: "completed" }))
          .status,
      ).toBe(404);
      const fireId = await fire();
      expect(
        (await post("/api/mock/pulse-fire/finish", { fire_id: fireId, status: "done" })).status,
      ).toBe(422);
    });
  });

  describe("actions", () => {
    it("lists the pending actions with their due times", async () => {
      const { status, body } = await fetchJson(url("/actions"));
      expect(status).toBe(200);
      expect((body as ActionInfo[]).map((a) => [a.id, a.name, a.current_run])).toEqual([
        ["act-7f3a2c", "weekly_digest", null],
        ["act-91be04", "review_open_prs", null],
      ]);
    });

    it("shows the live run of an action that has fired", async () => {
      startRun("action:weekly_digest");
      const { body } = await fetchJson(url("/actions"));
      expect((body as ActionInfo[])[0]?.current_run).toMatchObject({ run_id: "run-sched-1" });
    });

    it("cancels an action, and answers 404 once it is gone", async () => {
      const cancel = (): Promise<{ status: number; body: unknown }> =>
        fetchJson(url("/actions/act-7f3a2c"), { method: "DELETE" });
      expect(await cancel()).toEqual({
        status: 200,
        body: { id: "act-7f3a2c", cancelled: true },
      });
      expect(await cancel()).toEqual({
        status: 404,
        body: { error: 'No scheduled action with id "act-7f3a2c" was found.' },
      });
      const { body } = await fetchJson(url("/actions"));
      expect((body as ActionInfo[]).map((a) => a.id)).toEqual(["act-91be04"]);
    });
  });
});
