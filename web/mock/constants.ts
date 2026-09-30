import type { CloudStatusResponse } from "../src/lib/types";

/** Stand-in for `update::CURRENT_VERSION`, embedded the way the real artifacts listener does. */
export const MOCK_RESIDUUM_VERSION = "0.0.0-mock";

/** The detectable capabilities the mock implements (a subset of `src/features.rs`). */
export const MOCK_FEATURES: readonly string[] = [
  "model-complete",
  "artifact-sessions",
  "artifact-state",
];

/** What `GET /api/hub/cloud/status` reports, and the `tunnel` of `GET /api/hub/status`. */
export const MOCK_CLOUD_STATUS: CloudStatusResponse = {
  status: "disconnected",
  user_id: null,
  has_token: false,
  enabled: false,
  viewed_via_tunnel: false,
};
