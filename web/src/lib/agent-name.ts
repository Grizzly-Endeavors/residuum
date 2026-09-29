// Agent-name rules, mirrored from the backend (`validate_agent_name` in
// `src/config/paths.rs`): the name is the agent's directory name and A2A
// path segment.

const MAX_AGENT_NAME_LEN = 24;
const RESERVED_AGENT_NAMES: readonly string[] = ["hub", "team", "agents"];

/** The default name offered for the first agent. */
export const DEFAULT_AGENT_NAME = "assistant";

/**
 * Check a proposed agent name. Returns a plain-language problem, or null
 * when the name is acceptable.
 */
export function agentNameProblem(name: string): string | null {
  if (name.length === 0) return "Give your agent a name.";
  if (name.length > MAX_AGENT_NAME_LEN) {
    return `Use ${MAX_AGENT_NAME_LEN} characters or fewer.`;
  }
  if (!/^[a-z0-9-]+$/.test(name)) {
    return "Use only lowercase letters, digits, and hyphens.";
  }
  if (name.startsWith("-") || name.endsWith("-")) {
    return "The name can't start or end with a hyphen.";
  }
  if (RESERVED_AGENT_NAMES.includes(name)) {
    return `"${name}" is reserved. Pick a different name.`;
  }
  return null;
}
