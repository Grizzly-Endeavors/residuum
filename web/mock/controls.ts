import type { TurnHold } from "./env";
import { json, parseJsonObject, readBody, readJsonObject } from "./http";
import { offeredModelsOnly } from "./provider-models";
import type { Route, RouteContext } from "./routes";
import { changeAgentFile, changeTeamFile } from "./workspace-changes";

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
  const address = `agent:${from}`;
  agent.state.extraRecent.push(
    {
      role: "user",
      // The header and the structured sender the backend records for a teammate.
      content:
        `[Message from teammate ${address}, not the user. Your response in this turn is ` +
        `not shown to them; to reply, call message_agent with to="${address}".]\n` +
        "Can you look over the wiki index when you get a chance?",
      timestamp: now,
      visibility: "background",
      agent_sender: { address, category: "teammate" },
    },
    { role: "assistant", content: reply, timestamp: now, visibility: "user" },
  );
  agent.state.broadcast({
    type: "response",
    reply_to: "teammate",
    endpoint: "background",
    content: reply,
  });
  hub.teamEvents.agentReplied(agent);
  hub.overview.changed(agent);
  if (agent.connectedClients() === 0) hub.addUnread(agent);
  json(res, 200, { ok: true });
}

/**
 * A person writes to an agent (`?agent=atlas`) on Telegram, as `{ content?, name? }`
 * (a question about the routing doc, from Alex, by default): every connected
 * page is told of the message and then sees the agent work on it, and the
 * agent answers on Telegram. The turn is recorded in history with its sender.
 */
async function telegramMessage({ req, res, hub, query }: RouteContext): Promise<void> {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (!agent) {
    json(res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  const raw = await readBody(req);
  const {
    content = "Can you check what the routing doc says about urgent notices?",
    name = "Alex",
  } = raw.trim() === "" ? {} : parseJsonObject(raw);
  if (typeof content !== "string" || typeof name !== "string") {
    json(res, 422, { error: "mock: `content` and `name` must be strings" });
    return;
  }
  agent.receiveMessage(content, {
    endpoint: "telegram",
    sender: { name, id: "42", interface: "telegram", location: "direct message" },
  });
  json(res, 200, { ok: true });
}

/**
 * Fix the settings that stop an agent (`?agent=brittle`) starting, the way a
 * user would in Settings: every model its provider doesn't offer becomes the
 * provider's first one, so its next start succeeds. Its state doesn't change
 * until something starts it.
 */
function fixAgent({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (!agent) {
    json(res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  agent.state.providersToml = offeredModelsOnly(agent.state.providersToml);
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
 * `{ held }`: how far simulated turns may get. `true` stops each at its last
 * step instead of ending it, `"steps"` stops each while its file reads are
 * still running, and `false` lets them end. Easing the hold lets the waiting
 * turns carry on. Reset lifts it.
 */
async function holdTurns({ req, res, hub }: RouteContext): Promise<void> {
  const { held } = await readJsonObject(req);
  const holds = new Map<unknown, TurnHold>([
    [true, "end"],
    [false, "none"],
    ["steps", "steps"],
  ]);
  const hold = holds.get(held);
  if (hold === undefined) {
    json(res, 422, { error: 'mock: `held` must be true, false or "steps"' });
    return;
  }
  hub.env.holdTurns(hold);
  json(res, 200, { held });
}

/** `GET ?agent=name`: how many pages have the agent's chat socket open right now, as `{ pages }`. */
function connectedPages({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (agent === undefined) {
    json(res, 404, { error: "mock: no such agent" });
    return;
  }
  json(res, 200, { pages: agent.connectedClients() });
}

/**
 * `{ path, content }`: an agent writes the team file at `path` (`team/workbench/tip-splitter.html`),
 * or removes it, folders included, when `content` is `null`. The mock's files change and its sockets
 * send what the real system would (see `changeTeamFile`); the answer says what was sent.
 */
async function changeTeamFileControl({ req, res, hub }: RouteContext): Promise<void> {
  const change = await readFileChange(req, res);
  if (change === null) return;
  const outcome = changeTeamFile(hub, change.path, change.content);
  if ("status" in outcome) json(res, outcome.status, { error: outcome.error });
  else json(res, 200, outcome);
}

/**
 * `{ path, content }` with `?agent=atlas`: that agent writes the file at `path`
 * in its own workspace (`notes/today.md`), or removes it, folders included,
 * when `content` is `null`. Its socket sends `workspace_changed` to the pages
 * watching the path (see `changeAgentFile`); the answer says what was sent.
 */
async function changeAgentFileControl({ req, res, hub, query }: RouteContext): Promise<void> {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (!agent) {
    json(res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  const change = await readFileChange(req, res);
  if (change === null) return;
  const outcome = changeAgentFile(agent, change.path, change.content);
  if ("status" in outcome) json(res, outcome.status, { error: outcome.error });
  else json(res, 200, outcome);
}

/** A file change control's body, or `null` once it has answered `422` for one it can't use. */
async function readFileChange(
  req: RouteContext["req"],
  res: RouteContext["res"],
): Promise<{ path: string; content: string | null } | null> {
  const { path, content } = await readJsonObject(req);
  if (typeof path !== "string" || (typeof content !== "string" && content !== null)) {
    json(res, 422, {
      error: "mock: `path` must be a string, and `content` a string, or null to remove the file",
    });
    return null;
  }
  return { path, content };
}

/**
 * `{ devices }`: the push devices the hub would send no push right now,
 * because a connected page reported them in front of the user (`presence`,
 * kept on the mock clock) within the last minute.
 */
function presentPushDevices({ res, hub }: RouteContext): void {
  json(res, 200, { devices: hub.presentPushDevices() });
}

/**
 * The hub's session relay loses frames: every hub socket page that follows a
 * session is sent `session_relay_lagged`, as a connection that fell behind
 * the relay is. The answer says how many pages were told.
 */
function lagSessionRelay({ res, hub }: RouteContext): void {
  json(res, 200, { notified: hub.lagSessionRelay() });
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

/**
 * The app is rebuilt: a preview server serves its service worker as another
 * version from then on (see `createRebuiltWorkerHandler`), so a page that has
 * the old one finds an update. Answers `{ rebuilds }`, how many times so far.
 * Reset starts over from the build as it is.
 */
function rebuildApp({ res, hub }: RouteContext): void {
  hub.appRebuilds += 1;
  json(res, 200, { rebuilds: hub.appRebuilds });
}

/** The test control routes. */
export const controlRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/mock/rebuild", handler: rebuildApp },
  { method: "GET", pattern: "/api/mock/push/presence", handler: presentPushDevices },
  { method: "POST", pattern: "/api/mock/hub-socket", handler: hubSocketControl },
  { method: "POST", pattern: "/api/mock/team-file", handler: changeTeamFileControl },
  { method: "POST", pattern: "/api/mock/agent-file", handler: changeAgentFileControl },
  { method: "POST", pattern: "/api/mock/session-relay-lag", handler: lagSessionRelay },
  { method: "POST", pattern: "/api/mock/missed-relay", handler: missedRelay },
  { method: "POST", pattern: "/api/mock/teammate-message", handler: teammateMessage },
  { method: "POST", pattern: "/api/mock/telegram-message", handler: telegramMessage },
  { method: "POST", pattern: "/api/mock/fix-agent", handler: fixAgent },
  { method: "POST", pattern: "/api/mock/reset", handler: reset },
  { method: "POST", pattern: "/api/mock/clock/advance", handler: advanceClock },
  { method: "POST", pattern: "/api/mock/delays", handler: setDelays },
  { method: "POST", pattern: "/api/mock/turn-hold", handler: holdTurns },
  { method: "GET", pattern: "/api/mock/connected-pages", handler: connectedPages },
];
