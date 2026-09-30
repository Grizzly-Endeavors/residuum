import { json } from "./http";
import type { Route, RouteContext } from "./routes";

/**
 * Test controls: `POST /api/mock/...` endpoints that stage a situation for the
 * web UI to handle. They run against the agent named by `?agent=`, or the first
 * running one (see `scopeRequest`).
 */

/**
 * A session's result reaches main while the page is disconnected: record it
 * (and main's reply) in history, then drop the sockets. The page should show it
 * once it reconnects.
 */
function missedRelay({ res, state }: RouteContext): void {
  const now = new Date().toISOString();
  state.extraRecent.push(
    {
      role: "user",
      content:
        "[Agent Message from spawned-research-3f9a (spawned)]\nMissed while you were away: the fallback doc is drafted.",
      timestamp: now,
      visibility: "background",
      agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
    },
    {
      role: "assistant",
      content: "The research session finished the fallback doc while you were disconnected.",
      timestamp: now,
      visibility: "background",
    },
  );
  state.dropSockets();
  json(res, 200, { ok: true });
}

/**
 * A teammate (`?from=`, scout by default) messages an agent (`?agent=atlas`):
 * the message lands in its main conversation, and the hub reports it unread
 * until the web UI opens that agent's socket.
 */
function teammateMessage({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (!agent) {
    json(res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  const now = new Date().toISOString();
  const from = query.get("from") ?? "scout";
  const reply = `${from} asked me to check the wiki index. On it.`;
  agent.state.extraRecent.push(
    {
      role: "user",
      content: `[Message from ${from}]\nCan you look over the wiki index when you get a chance?`,
      timestamp: now,
      visibility: "user",
    },
    { role: "assistant", content: reply, timestamp: now, visibility: "user" },
  );
  agent.state.broadcast({ type: "response", reply_to: "teammate", content: reply });
  if (agent.connectedClients() === 0) hub.addUnread(agent);
  json(res, 200, { ok: true });
}

/** The test control routes. */
export const controlRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/mock/missed-relay", handler: missedRelay },
  { method: "POST", pattern: "/api/mock/teammate-message", handler: teammateMessage },
];
