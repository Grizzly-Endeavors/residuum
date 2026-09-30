// ── The viewed agent ─────────────────────────────────────────────────
//
// The agent the URL names. The router publishes it on every location change
// and the WebSocket coordinator binds to it. It is a signal between those two
// and nothing more: no request reads it. Every API call takes its agent as an
// argument (see `paths.ts`).

type AgentListener = (agent: string | null) => void;

let viewedAgent: string | null = null;

const agentListeners = new Set<AgentListener>();

/**
 * Publish `name` as the viewed agent. Listeners run before this returns, so
 * whatever holds the previous agent's state is torn down before anything can
 * address the new one.
 */
export function setViewedAgent(name: string | null): void {
  if (name === viewedAgent) return;
  viewedAgent = name;
  for (const listener of agentListeners) listener(name);
}

/** Observe the viewed agent changing. Returns a function that stops observing. */
export function onViewedAgentChange(listener: AgentListener): () => void {
  agentListeners.add(listener);
  return () => agentListeners.delete(listener);
}
