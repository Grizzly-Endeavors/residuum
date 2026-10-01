import { json, parseJsonObject, readBody, readJsonObject } from "./http";
import type { Route, RouteContext } from "./routes";
import { changeTeamFile } from "./team-changes";

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
  const now = state.env.clock.iso();
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
  const now = hub.env.clock.iso();
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
  hub.teamEvents.agentReplied(agent);
  if (agent.connectedClients() === 0) hub.addUnread(agent);
  json(res, 200, { ok: true });
}

/**
 * Put the mock back as it started (see `MockHub.reset`). Whatever a test did
 * is gone, and every connected page is dropped and reconnects to the initial
 * scenario. With `{ "setup": true }` the hub starts over with no agents, so
 * the web UI opens the setup wizard.
 */
async function reset({ req, res, hub }: RouteContext): Promise<void> {
  const raw = await readBody(req);
  const { setup = false } = raw.trim() === "" ? {} : parseJsonObject(raw);
  if (typeof setup !== "boolean") {
    json(res, 422, { error: "mock: `setup` must be true or false" });
    return;
  }
  hub.reset({ setup });
  json(res, 200, { ok: true });
}

/** `{ ms }`: move the clock forward, and answer with the time it now reads. */
async function advanceClock({ req, res, hub }: RouteContext): Promise<void> {
  const { ms } = await readJsonObject(req);
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) {
    json(res, 422, { error: "mock: `ms` must be a non-negative number of milliseconds" });
    return;
  }
  hub.env.clock.advance(ms);
  json(res, 200, { now: hub.env.clock.iso() });
}

/** `{ scale }`: how long simulated work takes now: `1` is the natural pace, `0` is instant. Reset restores the start-up value. */
async function setDelays({ req, res, hub }: RouteContext): Promise<void> {
  const { scale } = await readJsonObject(req);
  if (typeof scale !== "number" || !Number.isFinite(scale) || scale < 0) {
    json(res, 422, { error: "mock: `scale` must be a non-negative number" });
    return;
  }
  hub.env.setDelayScale(scale);
  json(res, 200, { scale });
}

/**
 * `{ path, content }`: an agent writes the team file at `path` (`team/workbench/tip-splitter.html`),
 * or removes it, folders included, when `content` is `null`. The mock's files change and its sockets
 * send what the real system would (see `changeTeamFile`); the answer says what was sent.
 */
async function changeTeamFileControl({ req, res, hub }: RouteContext): Promise<void> {
  const { path, content } = await readJsonObject(req);
  if (typeof path !== "string" || (typeof content !== "string" && content !== null)) {
    json(res, 422, {
      error: "mock: `path` must be a string, and `content` a string, or null to remove the file",
    });
    return;
  }
  const outcome = changeTeamFile(hub, path, content);
  if ("status" in outcome) json(res, outcome.status, { error: outcome.error });
  else json(res, 200, outcome);
}

/**
 * `{ online }`: take the hub WebSocket down (`false`), dropping every page and
 * refusing new connections, or let pages connect again (`true`). The HTTP API
 * stays up. Reset brings the socket back.
 */
async function hubSocketControl({ req, res, hub }: RouteContext): Promise<void> {
  const { online } = await readJsonObject(req);
  if (typeof online !== "boolean") {
    json(res, 422, { error: "mock: `online` must be true or false" });
    return;
  }
  hub.setHubSocketOnline(online);
  json(res, 200, { online });
}

/** The test control routes. */
export const controlRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/mock/hub-socket", handler: hubSocketControl },
  { method: "POST", pattern: "/api/mock/team-file", handler: changeTeamFileControl },
  { method: "POST", pattern: "/api/mock/missed-relay", handler: missedRelay },
  { method: "POST", pattern: "/api/mock/teammate-message", handler: teammateMessage },
  { method: "POST", pattern: "/api/mock/reset", handler: reset },
  { method: "POST", pattern: "/api/mock/clock/advance", handler: advanceClock },
  { method: "POST", pattern: "/api/mock/delays", handler: setDelays },
];
