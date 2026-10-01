import { MOCK_BRITTLE_FAILURE, MOCK_BRITTLE_PROVIDERS } from "./constants";
import type { MockHub } from "./state";

/**
 * Create the agents the mock starts with: `scout` and `atlas` running,
 * `drifter` stopped and never run, and `brittle` failed on a main model its
 * provider doesn't offer. brittle fails again whenever it is started until
 * its settings name a model the provider does offer, which Settings or
 * `POST /api/mock/fix-agent` can do.
 */
export function seedAgents(hub: MockHub): void {
  hub.createAgent("scout", { role: "Digs through the web and the wiki, then reports back" });
  hub.createAgent("atlas", { role: "Keeps the team wiki tidy" });
  hub.createAgent("drifter", { runState: "stopped", role: "Sleeps until something needs it" });
  const brittle = hub.createAgent("brittle", {
    runState: "failed",
    role: "Has a broken model config",
    lastError: MOCK_BRITTLE_FAILURE,
  });
  brittle.state.providersToml = MOCK_BRITTLE_PROVIDERS;
}
