import type { AgentLastError } from "../src/lib/generated/protocol";
import type { CloudStatusResponse } from "../src/lib/types";

/** Stand-in for `update::CURRENT_VERSION`, embedded the way the real artifacts listener does. */
export const MOCK_RESIDUUM_VERSION = "0.0.0-mock";

/** The detectable capabilities the mock implements (a subset of `src/features.rs`). */
export const MOCK_FEATURES: readonly string[] = [
  "model-complete",
  "artifact-sessions",
  "artifact-state",
];

/** The underlying error behind `brittle`'s failure, which `AgentLastError.reason` carries. */
export const MOCK_BRITTLE_REASON =
  "config error: providers.toml: model 'gpt-9' is not offered by provider 'openai'";

/** What `AgentLastError.message` says about it: the reason wrapped the way the hub wraps a start failure. */
export const MOCK_BRITTLE_ERROR = `brittle couldn't start: ${MOCK_BRITTLE_REASON}. Fix its settings or model configuration, then start it again.`;

/** Why `brittle`, the mock's agent with a broken model config, is failed, and fails every start. */
export const MOCK_BRITTLE_FAILURE: Omit<AgentLastError, "at"> = {
  message: MOCK_BRITTLE_ERROR,
  kind: "config",
  reason: MOCK_BRITTLE_REASON,
};

/** What `GET /api/hub/cloud/status` reports, and the `tunnel` of `GET /api/hub/status`. */
export const MOCK_CLOUD_STATUS: CloudStatusResponse = {
  status: "disconnected",
  user_id: null,
  has_token: false,
  enabled: false,
  viewed_via_tunnel: false,
};
