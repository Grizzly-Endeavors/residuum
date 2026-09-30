const MAX_AGENT_NAME_LEN = 24;
const RESERVED_NAMES = ["hub", "team", "agents"];

/** The backend's `validate_agent_name`: an error message, or `null` for a valid name. */
export function agentNameProblem(name: string): string | null {
  if (name === "") return "agent name must not be empty";
  if (name.length > MAX_AGENT_NAME_LEN) {
    return `agent name '${name}' is too long: at most ${MAX_AGENT_NAME_LEN} characters`;
  }
  if (!/^[a-z0-9-]+$/.test(name) || name.startsWith("-") || name.endsWith("-")) {
    return `agent name '${name}' must contain only lowercase letters, digits, and hyphens, and must not start or end with a hyphen`;
  }
  if (RESERVED_NAMES.includes(name)) {
    return `agent name '${name}' is reserved and cannot be used; reserved names: ${RESERVED_NAMES.join(", ")}`;
  }
  return null;
}
