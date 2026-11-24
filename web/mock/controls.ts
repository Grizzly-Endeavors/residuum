import { backgroundTurnFrames } from "./chat-scenarios";
import type { TimeProgress, TurnHold } from "./env";
import { json, parseJsonObject, readBody, readJsonObject } from "./http";
import { offeredModelsOnly } from "./provider-models";
import type { Route, RouteContext } from "./routes";
import { messageMainFrom } from "./sessions";
import { changeAgentFile, changeTeamFile } from "./workspace-changes";

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
  const turnId = `bg-${String(hub.env.nextId())}`;
  const content =
    `[Message from teammate ${address}, not the user. Your response in this turn is ` +
    `not shown to them; to reply, call message_agent with to="${address}".]\n` +
    "Can you look over the wiki index when you get a chance?";
  const agentSender = { address, category: "teammate" };
  agent.state.extraRecent.push(
    {
      role: "user",
      content,
      timestamp: now,
      visibility: "background",
      agent_sender: agentSender,
      turn_id: turnId,
    },
    {
      role: "assistant",
      content: reply,
      timestamp: now,
      visibility: "user",
      turn_id: turnId,
    },
  );
  agent.state.broadcast({
    type: "user_message",
    id: turnId,
    turn_id: turnId,
    content,
    agent_sender: agentSender,
    endpoint: "background",
  });
  for (const frame of backgroundTurnFrames(turnId, reply)) agent.state.broadcast(frame);
  hub.teamEvents.agentReplied(agent);
  hub.overview.changed(agent);
  if (agent.connectedClients() === 0) hub.addUnread(agent);
  json(res, 200, { ok: true });
}

/**
 * One of an agent's live sessions (`?address=`, the research session by
 * default) sends its main conversation `{ content? }`, as `message_agent` or
 * a relayed result does: it joins the turn in flight, announced as a
 * `user_message` naming the session and its run that the turn journal keeps,
 * or starts a turn. Answers `{ run_id }`, the sending run.
 */
async function sessionMessage({ req, res, state, query }: RouteContext): Promise<void> {
  const address = query.get("address") ?? "spawned-research-3f9a";
  const session = state.sessions.live.find((s) => s.address === address);
  if (session === undefined) {
    json(res, 404, { error: `mock: no live session at ${address}` });
    return;
  }
  const raw = await readBody(req);
  const { content = "Interim note: the cascade option needs a retry cap." } =
    raw.trim() === "" ? {} : parseJsonObject(raw);
  if (typeof content !== "string") {
    json(res, 422, { error: "mock: `content` must be a string" });
    return;
  }
  messageMainFrom(state, session, content);
  json(res, 200, { run_id: session.run_id });
}

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

function fixAgent({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (!agent) {
    json(res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  agent.state.providersToml = offeredModelsOnly(agent.state.providersToml);
  json(res, 200, { ok: true });
}

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

async function advanceClock({ req, res, hub }: RouteContext): Promise<void> {
  const { ms } = await readJsonObject(req);
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) {
    json(res, 422, { error: "mock: `ms` must be a non-negative number of milliseconds" });
    return;
  }
  hub.env.clock.advance(ms);
  json(res, 200, { now: hub.env.clock.iso() });
}

async function setDelays({ req, res, hub }: RouteContext): Promise<void> {
  const { scale } = await readJsonObject(req);
  if (typeof scale !== "number" || !Number.isFinite(scale) || scale < 0) {
    json(res, 422, { error: "mock: `scale` must be a non-negative number" });
    return;
  }
  hub.env.setDelayScale(scale);
  json(res, 200, { scale });
}

async function setTimeMode({ req, res, hub }: RouteContext): Promise<void> {
  const { mode } = await readJsonObject(req);
  if (mode !== "manual" && mode !== "scaled") {
    json(res, 422, { error: 'mock: `mode` must be "manual" or "scaled"' });
    return;
  }
  hub.env.setTimeMode(mode);
  json(res, 200, { mode, pending: hub.env.pendingTimers(), ...readings(hub) });
}

function readings(hub: RouteContext["hub"]): { now: string; next: string | null } {
  const nextIn = hub.env.nextTimerIn();
  return {
    now: hub.env.clock.iso(),
    next: nextIn === null ? null : hub.env.clock.isoIn(nextIn),
  };
}

function timeMoved(
  res: RouteContext["res"],
  hub: RouteContext["hub"],
  progress: TimeProgress,
): void {
  json(res, 200, { ...progress, ...readings(hub) });
}

async function advanceTime({ req, res, hub }: RouteContext): Promise<void> {
  const { ms } = await readJsonObject(req);
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) {
    json(res, 422, { error: "mock: `ms` must be a non-negative number of milliseconds" });
    return;
  }
  if (hub.env.timeMode() !== "manual") {
    json(res, 409, { error: "mock: time is scaled; set manual time first" });
    return;
  }
  timeMoved(res, hub, await hub.env.advance(ms));
}

async function stepTime({ res, hub }: RouteContext): Promise<void> {
  if (hub.env.timeMode() !== "manual") {
    json(res, 409, { error: "mock: time is scaled; set manual time first" });
    return;
  }
  timeMoved(res, hub, await hub.env.step());
}

async function holdTurns({ req, res, hub }: RouteContext): Promise<void> {
  const { held } = await readJsonObject(req);
  const holds = new Map<unknown, TurnHold>([
    [true, "end"],
    [false, "none"],
    ["steps", "steps"],
    ["reply", "reply"],
  ]);
  const hold = holds.get(held);
  if (hold === undefined) {
    json(res, 422, { error: 'mock: `held` must be true, false, "steps" or "reply"' });
    return;
  }
  hub.env.holdTurns(hold);
  json(res, 200, { held });
}

function connectedPages({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (agent === undefined) {
    json(res, 404, { error: "mock: no such agent" });
    return;
  }
  json(res, 200, { pages: agent.connectedClients() });
}

function watchedPrefixes({ res, hub, query }: RouteContext): void {
  const agent = hub.agents.get(query.get("agent") ?? "");
  if (agent === undefined) {
    json(res, 404, { error: "mock: no such agent" });
    return;
  }
  json(res, 200, { prefixes: agent.watchedPrefixes() });
}

async function changeTeamFileControl({ req, res, hub }: RouteContext): Promise<void> {
  const change = await readFileChange(req, res);
  if (change === null) return;
  const outcome = changeTeamFile(hub, change.path, change.content);
  if ("status" in outcome) json(res, outcome.status, { error: outcome.error });
  else json(res, 200, outcome);
}

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

function presentPushDevices({ res, hub }: RouteContext): void {
  json(res, 200, { devices: hub.presentPushDevices() });
}

function lagSessionRelay({ res, hub }: RouteContext): void {
  json(res, 200, { notified: hub.lagSessionRelay() });
}

function failNextSessionStart({ res, state }: RouteContext): void {
  state.sessions.failNextOwnerStart = true;
  json(res, 200, { armed: true });
}

async function hubSocketControl({ req, res, hub }: RouteContext): Promise<void> {
  const { online } = await readJsonObject(req);
  if (typeof online !== "boolean") {
    json(res, 422, { error: "mock: `online` must be true or false" });
    return;
  }
  hub.setHubSocketOnline(online);
  json(res, 200, { online });
}

function rebuildApp({ res, hub }: RouteContext): void {
  hub.appRebuilds += 1;
  json(res, 200, { rebuilds: hub.appRebuilds });
}

export const controlRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/mock/rebuild", handler: rebuildApp },
  { method: "GET", pattern: "/api/mock/push/presence", handler: presentPushDevices },
  { method: "POST", pattern: "/api/mock/hub-socket", handler: hubSocketControl },
  { method: "POST", pattern: "/api/mock/team-file", handler: changeTeamFileControl },
  { method: "POST", pattern: "/api/mock/agent-file", handler: changeAgentFileControl },
  { method: "POST", pattern: "/api/mock/session-relay-lag", handler: lagSessionRelay },
  { method: "POST", pattern: "/api/mock/missed-relay", handler: missedRelay },
  { method: "POST", pattern: "/api/mock/fail-next-session-start", handler: failNextSessionStart },
  { method: "POST", pattern: "/api/mock/teammate-message", handler: teammateMessage },
  { method: "POST", pattern: "/api/mock/session-message", handler: sessionMessage },
  { method: "POST", pattern: "/api/mock/telegram-message", handler: telegramMessage },
  { method: "POST", pattern: "/api/mock/fix-agent", handler: fixAgent },
  { method: "POST", pattern: "/api/mock/reset", handler: reset },
  { method: "POST", pattern: "/api/mock/clock/advance", handler: advanceClock },
  { method: "POST", pattern: "/api/mock/delays", handler: setDelays },
  { method: "POST", pattern: "/api/mock/time", handler: setTimeMode },
  { method: "POST", pattern: "/api/mock/time/advance", handler: advanceTime },
  { method: "POST", pattern: "/api/mock/time/step", handler: stepTime },
  { method: "POST", pattern: "/api/mock/turn-hold", handler: holdTurns },
  { method: "GET", pattern: "/api/mock/connected-pages", handler: connectedPages },
  { method: "GET", pattern: "/api/mock/watches", handler: watchedPrefixes },
];
