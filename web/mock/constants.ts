import type { AgentLastError } from "../src/lib/generated/protocol";

/** Stand-in for `update::CURRENT_VERSION`, embedded the way the real artifacts listener does. */
export const MOCK_RESIDUUM_VERSION = "0.0.0-mock";

/** The `agentName` of the state that holds hub-level and team-level data. */
export const HUB_STATE_NAME = "hub";

/** The boot id the hub announces in deterministic mode, where a random one would differ between runs. */
export const MOCK_DETERMINISTIC_BOOT_ID = "00000000-0000-4000-8000-000000000000";

/** The detectable capabilities the mock implements (a subset of `src/features.rs`). */
export const MOCK_FEATURES: readonly string[] = [
  "model-complete",
  "artifact-sessions",
  "artifact-state",
];

/**
 * Why an agent couldn't start because of a problem in its `providers.toml`:
 * the problem as the reason, wrapped in the message the way the hub wraps a
 * start failure.
 */
export function providersStartFailure(agent: string, problem: string): Omit<AgentLastError, "at"> {
  const reason = `config error: providers.toml: ${problem}`;
  return {
    message: `${agent} couldn't start: ${reason}. Fix its settings or model configuration, then start it again.`,
    kind: "config",
    reason,
  };
}

/** `brittle`'s `providers.toml`: a main model its provider doesn't offer, so every start fails until that changes. */
export const MOCK_BRITTLE_PROVIDERS = '[models]\nmain = "openai/gpt-9"\n';

/** Why `brittle` is failed, and fails every start. */
export const MOCK_BRITTLE_FAILURE = providersStartFailure(
  "brittle",
  "model 'gpt-9' is not offered by provider 'openai'",
);

/** The underlying error behind `brittle`'s failure, which `AgentLastError.reason` carries. */
export const MOCK_BRITTLE_REASON = MOCK_BRITTLE_FAILURE.reason;

/** What `AgentLastError.message` says about it. */
export const MOCK_BRITTLE_ERROR = MOCK_BRITTLE_FAILURE.message;
