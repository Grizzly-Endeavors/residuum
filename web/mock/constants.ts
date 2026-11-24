import type { AgentLastError } from "../src/lib/generated/protocol";

export const MOCK_RESIDUUM_VERSION = "0.0.0-mock";

export const HUB_STATE_NAME = "hub";

export const MOCK_DETERMINISTIC_BOOT_ID = "00000000-0000-4000-8000-000000000000";

export const MOCK_FEATURES: readonly string[] = [
  "model-complete",
  "artifact-sessions",
  "artifact-state",
];

export function providersStartFailure(agent: string, problem: string): Omit<AgentLastError, "at"> {
  const reason = `config error: providers.toml: ${problem}`;
  return {
    message: `${agent} couldn't start: ${reason}. Fix its settings or model configuration, then start it again.`,
    kind: "config",
    reason,
  };
}

export const MOCK_BRITTLE_PROVIDERS = '[models]\nmain = "openai/gpt-9"\n';

export const MOCK_BRITTLE_FAILURE = providersStartFailure(
  "brittle",
  "model 'gpt-9' is not offered by provider 'openai'",
);

export const MOCK_BRITTLE_REASON = MOCK_BRITTLE_FAILURE.reason;

export const MOCK_BRITTLE_ERROR = MOCK_BRITTLE_FAILURE.message;
