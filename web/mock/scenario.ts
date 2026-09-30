import { MOCK_BRITTLE_ERROR } from "./constants";
import type { MockHub } from "./state";

/**
 * Create the agents the mock starts with: `scout` and `atlas` running,
 * `drifter` stopped and never run, and `brittle` failed on a broken model
 * config, which fails again whenever it is started.
 */
export function seedAgents(hub: MockHub): void {
  hub.createAgent("scout", { role: "Digs through the web and the wiki, then reports back" });
  hub.createAgent("atlas", { role: "Keeps the team wiki tidy" });
  hub.createAgent("drifter", { runState: "stopped", role: "Sleeps until something needs it" });
  hub.createAgent("brittle", {
    runState: "failed",
    role: "Has a broken model config",
    lastError: MOCK_BRITTLE_ERROR,
  });
}
