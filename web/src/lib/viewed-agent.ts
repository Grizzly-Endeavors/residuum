type AgentListener = (agent: string | null) => void;

let viewedAgent: string | null = null;

const agentListeners = new Set<AgentListener>();

export function setViewedAgent(name: string | null): void {
  if (name === viewedAgent) return;
  viewedAgent = name;
  for (const listener of agentListeners) listener(name);
}

export function onViewedAgentChange(listener: AgentListener): () => void {
  agentListeners.add(listener);
  return () => agentListeners.delete(listener);
}
