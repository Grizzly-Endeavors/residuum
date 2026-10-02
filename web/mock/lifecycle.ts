import type { AgentSummary, DeleteOutcome } from "../src/lib/generated/protocol";
import type {
  DeletedAgentListResponse,
  HubStatusResponse,
  StopAllResponse,
} from "../src/lib/hub-types";
import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import { agentNameProblem } from "./agent-name";
import { checkpointBeforeAction } from "./checkpoints";
import { cloudStatusOf } from "./cloud";
import { MOCK_RESIDUUM_VERSION, providersStartFailure } from "./constants";
import { json, readJsonObject, stringField, type JsonObject } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import { modelProblems } from "./provider-models";
import type { MockAgent, MockHub } from "./state";
import { byName } from "./util";

/** How long an agent takes to start. */
const STARTUP_MS = 400;

/** How long a running agent takes to stop, which it spends listed as stopping. */
const STOP_MS = 250;

/** The backend's error for a body it can't use (`parse_body` in `src/hub/http/lifecycle.rs`). */
function badBodyMessage(err: unknown): string {
  return `the request body isn't valid for this route: ${err instanceof Error ? err.message : String(err)}`;
}

/**
 * Read a JSON object body. A body that isn't one, or that lacks the string
 * field `required`, is answered `400` and `null` comes back.
 */
async function readBodyOr400(ctx: RouteContext, required?: string): Promise<JsonObject | null> {
  try {
    const body = await readJsonObject(ctx.req);
    if (required !== undefined && typeof body[required] !== "string") {
      throw new Error(`missing field \`${required}\``);
    }
    return body;
  } catch (err) {
    json(ctx.res, 400, { error: badBodyMessage(err) });
    return null;
  }
}

/** The agent the route's first capture group names, or `null` after answering `404`. */
function namedAgent(ctx: RouteContext): MockAgent | null {
  const name = decodedParam(ctx, 0);
  const agent = ctx.hub.agents.get(name);
  if (!agent) json(ctx.res, 404, { error: `no agent named '${name}'` });
  return agent ?? null;
}

function sortedAgents(ctx: RouteContext): MockAgent[] {
  return [...ctx.hub.agents.values()].sort((a, b) => byName(a.name, b.name));
}

function listAgents(ctx: RouteContext): void {
  json(ctx.res, 200, ctx.hub.listing());
}

/** Newest deletion first, like the backend. */
function listDeletedAgents(ctx: RouteContext): void {
  const agents = [...ctx.hub.deleted.values()]
    .sort((a, b) => byName(b.deletedAt, a.deletedAt) || byName(a.agent.name, b.agent.name))
    .map((gone) => ({
      name: gone.agent.name,
      deleted_at: gone.deletedAt,
      checkpoint_id: gone.checkpointId,
    }));
  json(ctx.res, 200, { agents } satisfies DeletedAgentListResponse);
}

/** `POST /api/hub/agents/restore`: `{ name, checkpoint_id? }`; answers `201` with the summary. */
async function restoreAgent(ctx: RouteContext): Promise<void> {
  const { res, hub } = ctx;
  const body = await readBodyOr400(ctx, "name");
  if (body === null) return;
  const name = stringField(body, "name") ?? "";
  const nameProblem = agentNameProblem(name);
  if (nameProblem !== null) {
    json(res, 400, { error: nameProblem });
    return;
  }
  if (hub.agents.has(name)) {
    json(res, 409, { error: `an agent named '${name}' already exists` });
    return;
  }
  const gone = hub.deleted.get(name);
  if (!gone) {
    json(res, 404, { error: `there is no deleted agent named '${name}' to restore` });
    return;
  }
  const checkpointId = stringField(body, "checkpoint_id");
  if (checkpointId !== undefined && checkpointId !== gone.checkpointId) {
    json(res, 400, { error: `${name} has no checkpoint '${checkpointId}' to restore from` });
    return;
  }
  hub.deleted.delete(name);
  const { agent } = gone;
  agent.runState = agent.autostart ? "running" : "stopped";
  hub.agents.set(name, agent);
  hub.broadcast({ type: "agent_restored", agent: hub.summary(agent), by: "user" });
  json(res, 201, hub.summary(agent));
}

/** `POST /api/hub/agents`: `CreateAgentRequest`, where `name` is required and the rest may be absent or null. */
async function createAgent(ctx: RouteContext): Promise<void> {
  const { res, hub } = ctx;
  const body = await readBodyOr400(ctx, "name");
  if (body === null) return;
  const name = stringField(body, "name") ?? "";
  const nameProblem = agentNameProblem(name);
  if (nameProblem !== null) {
    json(res, 400, { error: nameProblem });
    return;
  }
  if (hub.agents.has(name)) {
    json(res, 409, { error: `an agent named '${name}' already exists` });
    return;
  }
  const modelsFrom = stringField(body, "models_from") ?? null;
  if (modelsFrom !== null && !hub.agents.has(modelsFrom)) {
    json(res, 400, { error: `no agent named '${modelsFrom}'` });
    return;
  }
  if (modelsFrom === null && typeof body.providers_toml !== "string") {
    json(res, 400, { error: "give models_from or providers_toml" });
    return;
  }
  // A new agent replaces the deleted one of that name, whose socket route would answer for it too.
  hub.deleted.get(name)?.agent.dispose();
  hub.deleted.delete(name);
  const agent = hub.createAgent(name, { role: stringField(body, "description") ?? null });
  if (body.a2a_visibility === "public") agent.visibility = "public";
  hub.broadcast({ type: "agent_created", agent: hub.summary(agent), by: "user" });
  json(res, 201, hub.summary(agent));
}

/** `GET /api/hub/status`: version, uptime, tunnel, and how many agents are in each state. */
function hubStatus(ctx: RouteContext): void {
  const counts = { starting: 0, running: 0, stopped: 0, failed: 0 };
  for (const agent of ctx.hub.agents.values()) counts[agent.runState]++;
  json(ctx.res, 200, {
    version: MOCK_RESIDUUM_VERSION,
    uptime_secs: Math.floor(ctx.hub.env.clock.elapsedMs() / 1000),
    tunnel: cloudStatusOf(ctx.hub.hubState),
    agents: counts,
  } satisfies HubStatusResponse);
}

/** The stops in progress, so a second request to stop an agent waits for the first, as the hub's per-agent lock makes it. */
const stopsInProgress = new WeakMap<MockAgent, Promise<void>>();

/**
 * Stop an agent. A running agent is announced as stopping, winds down for a
 * moment, and is then reported stopped. Asking again while it winds down waits
 * for that stop.
 */
function stopAgent(hub: MockHub, agent: MockAgent): Promise<void> {
  const inProgress = stopsInProgress.get(agent);
  if (inProgress !== undefined) return inProgress;
  const stop = (async (): Promise<void> => {
    if (agent.runState === "running") {
      hub.markStopping(agent);
      await hub.env.sleep(STOP_MS);
    }
    hub.transition(agent, "stopped");
  })().finally(() => {
    stopsInProgress.delete(agent);
  });
  stopsInProgress.set(agent, stop);
  return stop;
}

/** `POST /api/hub/stop-all`: stop every running or starting agent and leave the hub running. */
async function stopAll(ctx: RouteContext): Promise<void> {
  const { hub } = ctx;
  const stopped: AgentSummary[] = [];
  for (const agent of sortedAgents(ctx)) {
    if (agent.runState !== "running" && agent.runState !== "starting") continue;
    await stopAgent(hub, agent);
    stopped.push(hub.summary(agent));
  }
  json(ctx.res, 200, { stopped, failed: [] } satisfies StopAllResponse);
}

/**
 * `DELETE /api/hub/agents/{name}`: stop, checkpoint and remove an agent, keeping it for a restore.
 * A running agent stops the way `stopAgent` stops it, so it is listed as stopping first.
 */
async function deleteAgent(ctx: RouteContext): Promise<void> {
  const agent = namedAgent(ctx);
  if (agent === null) return;
  const { hub } = ctx;
  if (agent.runState === "running") await stopAgent(hub, agent);
  // Another delete of the same agent finished while this one waited for it to stop.
  if (hub.agents.get(agent.name) !== agent) {
    json(ctx.res, 404, { error: `no agent named '${agent.name}'` });
    return;
  }
  agent.state.dropSockets();
  agent.runState = "stopped";
  agent.busySince = null;
  agent.stopping = false;
  hub.agents.delete(agent.name);
  const checkpointId = `ckpt-${agent.name}-${String(hub.env.nextId())}`;
  hub.deleted.set(agent.name, { agent, deletedAt: hub.env.clock.iso(), checkpointId });
  hub.broadcast({ type: "agent_deleted", name: agent.name, by: "user" });
  json(ctx.res, 200, { deleted: true, checkpoint_id: checkpointId } satisfies DeleteOutcome);
}

/** `PATCH /api/hub/agents/{name}`: change autostart and/or A2A visibility. */
async function patchAgent(ctx: RouteContext): Promise<void> {
  const agent = namedAgent(ctx);
  if (agent === null) return;
  const body = await readBodyOr400(ctx);
  if (body === null) return;
  const { autostart, a2a_visibility: visibility } = body;
  const newVisibility = visibility === "public" || visibility === "private" ? visibility : null;
  if (typeof autostart !== "boolean" && newVisibility === null) {
    json(ctx.res, 400, {
      error: "the request must set at least one of autostart or a2a_visibility",
    });
    return;
  }
  if (typeof autostart === "boolean") agent.autostart = autostart;
  if (newVisibility !== null) agent.visibility = newVisibility;
  // The hub writes both to the agent's config.toml, checkpointing it first.
  const { state } = agent;
  checkpointBeforeAction(state, "agent_config", "patch config.toml");
  const doc = state.configToml.trim() ? parseToml(state.configToml) : {};
  if (typeof autostart === "boolean") doc.autostart = autostart;
  if (newVisibility !== null) {
    const a2a = doc.a2a;
    const table = typeof a2a === "object" && !Array.isArray(a2a) && !(a2a instanceof Date);
    doc.a2a = { ...(table ? a2a : {}), visibility: newVisibility };
  }
  state.configToml = stringifyToml(doc);
  ctx.hub.broadcast({ type: "agent_state", agent: ctx.hub.summary(agent) });
  json(ctx.res, 200, ctx.hub.summary(agent));
}

/**
 * `POST /api/hub/agents/{name}/(start|stop|restart)`. A start takes a moment,
 * and fails while the agent's `providers.toml` names a model its provider
 * doesn't offer, as brittle's does.
 */
async function runAgentAction(ctx: RouteContext): Promise<void> {
  const agent = namedAgent(ctx);
  if (agent === null) return;
  const { hub, res } = ctx;
  const action = ctx.params[1];
  if (action === "stop") {
    await stopAgent(hub, agent);
  } else if (action !== "start" || agent.runState !== "running") {
    // A restart stops the agent first, like the hub.
    if (action === "restart" && agent.runState === "running") await stopAgent(hub, agent);
    hub.transition(agent, "starting");
    await hub.env.sleep(STARTUP_MS);
    const [problem] = modelProblems(agent.state.providersToml);
    if (problem !== undefined) {
      const failure = providersStartFailure(agent.name, problem.message);
      agent.lastError = { ...failure, at: hub.env.clock.iso() };
      hub.transition(agent, "failed");
    } else {
      hub.transition(agent, "running");
    }
  }
  json(res, 200, hub.summary(agent));
}

/** The agent lifecycle routes and hub status, in the `/api/hub/...` spelling the hub keeps. */
export const lifecycleRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/hub/agents", handler: listAgents },
  { method: "GET", pattern: "/api/hub/agents/deleted", handler: listDeletedAgents },
  { method: "POST", pattern: "/api/hub/agents/restore", handler: restoreAgent },
  { method: "POST", pattern: "/api/hub/agents", handler: createAgent },
  { method: "GET", pattern: "/api/hub/status", handler: hubStatus },
  { method: "POST", pattern: "/api/hub/stop-all", handler: stopAll },
  { method: "DELETE", pattern: /^\/api\/hub\/agents\/([^/]+)$/, handler: deleteAgent },
  { method: "PATCH", pattern: /^\/api\/hub\/agents\/([^/]+)$/, handler: patchAgent },
  {
    method: "POST",
    pattern: /^\/api\/hub\/agents\/([^/]+)\/(start|stop|restart)$/,
    handler: runAgentAction,
  },
];
