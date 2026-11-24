import { MOCK_BRITTLE_FAILURE, MOCK_BRITTLE_PROVIDERS } from "./constants";
import { seedMail } from "./data/mail";
import type { MockHub } from "./state";

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
  seedMail(hub);
}
