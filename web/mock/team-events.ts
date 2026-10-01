import type {
  AgentState,
  ServerMessage,
  SessionRunStatus,
  SessionSummary,
} from "../src/lib/generated/protocol";
import type {
  Actor,
  AgentSummary,
  HubServerMessage,
  TeamEvent,
  TeamEventKind,
  TeamEventLevel,
  TeamEventPage,
  TeamEventTarget,
} from "../src/lib/hub-types";
import type { UserInboxItem } from "../src/lib/types";
import type { MockEnv } from "./env";
import { json, parseJsonObject, readBody, stringField } from "./http";
import type { Route, RouteContext } from "./routes";
import type { MockAgent, MockState } from "./state";

/**
 * The team event log of the hub, `src/hub/team_events/`: what happened across
 * the team since the hub started, in memory. The mock records the same
 * entries for the same things the backend does, from the frames its own
 * lifecycle, sessions, turns and inbox additions send, and words them the
 * way the backend's recorder does.
 */

/** How many entries the log holds. */
export const LOG_CAPACITY = 500;

/** How many of the newest `warn` and `error` entries survive eviction. */
export const PROTECTED_ENTRIES = 100;

/** How many entries a page holds when the request names no limit. */
export const DEFAULT_PAGE_SIZE = 50;

/** The most entries a page holds; a larger limit is treated as this. */
export const MAX_PAGE_SIZE = 200;

/** An entry that has not been given its id yet. */
export type NewTeamEvent = Omit<TeamEvent, "id">;

/** Which entries a page holds. Both bounds are exclusive. */
export interface PageQuery {
  before?: number;
  after?: number;
  limit?: number;
}

export interface MockTeamEvents {
  /** The id of the hub process the log belongs to, which `hub_boot` announces too. */
  bootId: string;
  /** Add an entry, evicting the oldest unprotected one when the log is full, and send it to hub clients as `team_event`. */
  record: (entry: NewTeamEvent) => TeamEvent;
  /** One page of entries, newest first, like `GET /api/hub/events`. */
  page: (query: PageQuery) => TeamEventPage;
  /**
   * Start over as a hub that has just started: an empty log, `hub_started`,
   * and what starting the agents in `agents` (by name) did, which is a start
   * for each running one and a failure for each failed one.
   */
  begin: (agents: readonly AgentSummary[]) => void;
  /** Read a frame the hub sends to its clients, as the backend's recorder reads the hub bus. */
  observeHub: (frame: HubServerMessage) => void;
  /** Read the session frames an agent sends to its clients. */
  watchAgent: (agent: MockAgent) => void;
  /** A main turn of the agent ended with a reply for the user. */
  agentReplied: (agent: MockAgent) => void;
  /** The agent's `user_inbox_add` tool saved the item `itemId`. */
  userInboxAdded: (agent: MockAgent, itemId: string) => void;
}

// ─── Wording ──────────────────────────────────────────────────────────────────
// Kept in step with `src/hub/team_events/recorder.rs`.

/** `text` on one line, with its runs of whitespace collapsed. */
function plain(text: string): string {
  return text
    .split(/\s+/)
    .filter((word) => word !== "")
    .join(" ");
}

/** `base`, then a colon and `detail` when there is one. */
function withDetail(base: string, detail: string): string {
  return detail === "" ? base : `${base}: ${detail}`;
}

/** "atlas was created", or "atlas was created by scout" when an agent did it. */
function withActor(name: string, done: string, by: Actor): string {
  return by === "user" ? `${name} ${done}` : `${name} ${done} by ${by.slice("agent:".length)}`;
}

/** What went wrong with an agent, for its `agent_failed` entry; `before` is the state it failed out of. */
function failureSummary(agent: AgentSummary, before: AgentState): string {
  const error = agent.last_error;
  if (error === null) return `${agent.name} failed`;
  if (error.kind === "crash") {
    return before === "running"
      ? `${agent.name} stopped unexpectedly`
      : `${agent.name} couldn't start because of an internal error`;
  }
  return `${agent.name} couldn't start: ${plain(error.reason).replace(/\.+$/, "")}`;
}

/** The level and summary of a finished session that was not scheduled. */
function sessionFinished(
  agent: string,
  session: SessionSummary,
  status: SessionRunStatus,
  error: string | null,
): { level: TeamEventLevel; summary: string } {
  const purpose = plain(session.purpose);
  if (status === "completed") {
    return { level: "info", summary: withDetail(`${agent} finished a session`, purpose) };
  }
  if (status === "cancelled") {
    return { level: "warn", summary: withDetail(`${agent}'s session was stopped`, purpose) };
  }
  return {
    level: "error",
    summary: withDetail(`${agent}'s session failed`, plain(error ?? "")),
  };
}

/** What a scheduled run is called, by the word before the colon of its source label. */
const SCHEDULED_KINDS: Readonly<Record<string, string>> = {
  pulse: "pulse",
  action: "scheduled action",
};

/** `pulse "email_check"`, `scheduled action "nightly digest"`: what a scheduled run was. */
function scheduledRunName(session: SessionSummary): string {
  const label = session.source_label;
  const colon = label.indexOf(":");
  const kind = SCHEDULED_KINDS[colon === -1 ? "" : label.slice(0, colon)] ?? "scheduled run";
  return `${kind} "${colon === -1 ? label : label.slice(colon + 1)}"`;
}

/** The level and summary of a finished pulse or scheduled action. */
function scheduledRunFinished(
  agent: string,
  session: SessionSummary,
  status: SessionRunStatus,
  error: string | null,
): { level: TeamEventLevel; summary: string } {
  const what = scheduledRunName(session);
  if (status === "completed") return { level: "info", summary: `${agent} finished the ${what}` };
  if (status === "cancelled") return { level: "info", summary: `${agent}'s ${what} was stopped` };
  return { level: "error", summary: withDetail(`${agent}'s ${what} failed`, plain(error ?? "")) };
}

function chatOf(agent: string): TeamEventTarget {
  return { kind: "agent_place", agent, place: "chat" };
}

function sessionTarget(agent: string, runId: string): TeamEventTarget {
  return { kind: "session", agent, run_id: runId };
}

// ─── The log ──────────────────────────────────────────────────────────────────

/**
 * The position of the oldest entry eviction may remove: any `info` entry, and
 * any `warn` or `error` entry older than the newest `PROTECTED_ENTRIES` of them.
 */
function oldestUnprotected(list: readonly TeamEvent[]): number {
  const problems = list.filter((event) => event.level !== "info");
  const floor = problems.at(-PROTECTED_ENTRIES)?.id ?? null;
  const found = list.findIndex(
    (event) => event.level === "info" || (floor !== null && event.id < floor),
  );
  return Math.max(found, 0);
}

/**
 * A team event log whose ids count from 1 and whose times follow `env`'s
 * clock, so a deterministic mock gives the same entries for the same steps.
 * `send` delivers a frame to every connected hub client.
 */
export function createTeamEvents(
  env: MockEnv,
  bootId: string,
  send: (frame: HubServerMessage) => void,
): MockTeamEvents {
  const entries: TeamEvent[] = [];
  let nextId = 1;
  /** The state each agent was last reported in; an agent never reported is stopped. */
  const states = new Map<string, AgentState>();

  function record(entry: NewTeamEvent): TeamEvent {
    if (entries.length >= LOG_CAPACITY) entries.splice(oldestUnprotected(entries), 1);
    const event: TeamEvent = { id: nextId, ...entry };
    nextId += 1;
    entries.push(event);
    send({ type: "team_event", boot_id: bootId, event });
    return event;
  }

  function recordInChat(
    agent: string,
    kind: TeamEventKind,
    level: TeamEventLevel,
    summary: string,
  ): void {
    record({
      at: env.clock.iso(),
      agent,
      kind,
      level,
      summary,
      target: chatOf(agent),
    });
  }

  /** An agent's state, autostart or visibility was sent: only a move into running, stopped or failed is told. */
  function onState(agent: AgentSummary): void {
    const before = states.get(agent.name) ?? "stopped";
    states.set(agent.name, agent.state);
    if (before === agent.state) return;
    if (agent.state === "running") {
      recordInChat(agent.name, "agent_started", "info", `${agent.name} started`);
    } else if (agent.state === "stopped" && (before === "running" || before === "starting")) {
      recordInChat(agent.name, "agent_stopped", "info", `${agent.name} stopped`);
    } else if (agent.state === "failed") {
      recordInChat(agent.name, "agent_failed", "error", failureSummary(agent, before));
    }
  }

  function observeHub(frame: HubServerMessage): void {
    if (frame.type === "agent_state") {
      onState(frame.agent);
    } else if (frame.type === "agent_created" || frame.type === "agent_restored") {
      // The hub starts the agent before it announces it, so the start comes first.
      onState(frame.agent);
      const { name } = frame.agent;
      const created = frame.type === "agent_created";
      recordInChat(
        name,
        created ? "agent_created" : "agent_restored",
        "info",
        withActor(name, created ? "was created" : "was restored", frame.by),
      );
    } else if (frame.type === "agent_deleted") {
      states.delete(frame.name);
      record({
        at: env.clock.iso(),
        agent: frame.name,
        kind: "agent_deleted",
        level: "info",
        summary: withActor(frame.name, "was deleted", frame.by),
        target: null,
      });
    } else if (frame.type === "notice") {
      record({
        at: env.clock.iso(),
        agent: frame.agent ?? null,
        kind: "hub_notice",
        level: frame.level,
        summary: plain(frame.message),
        target: null,
      });
    }
  }

  function onSessionStarted(agent: string, session: SessionSummary): void {
    // A scheduled run is told once, when it finishes.
    if (session.category === "scheduled") return;
    record({
      at: session.started_at,
      agent,
      kind: "session_started",
      level: "info",
      summary: withDetail(`${agent} started a session`, plain(session.purpose)),
      target: sessionTarget(agent, session.run_id),
    });
  }

  function onSessionCompleted(
    agent: MockAgent,
    frame: Extract<ServerMessage, { type: "session_completed" }>,
  ): void {
    const { sessions } = agent.state;
    const session = [...sessions.completed, ...sessions.live].find(
      (candidate) => candidate.address === frame.address && candidate.run_id === frame.run_id,
    );
    if (session === undefined) return;
    const scheduled = session.category === "scheduled";
    const { level, summary } = scheduled
      ? scheduledRunFinished(agent.name, session, frame.status, frame.error)
      : sessionFinished(agent.name, session, frame.status, frame.error);
    record({
      at: env.clock.iso(),
      agent: agent.name,
      kind: scheduled ? "scheduled_run_finished" : "session_finished",
      level,
      summary,
      target: sessionTarget(agent.name, frame.run_id),
    });
  }

  return {
    bootId,
    record,
    page: ({ before, after, limit }) => {
      const size = Math.min(Math.max(limit ?? DEFAULT_PAGE_SIZE, 1), MAX_PAGE_SIZE);
      const matching = entries
        .filter(
          (event) =>
            (before === undefined || event.id < before) &&
            (after === undefined || event.id > after),
        )
        .reverse();
      const events = matching.slice(0, size);
      const oldest = events.at(-1);
      return {
        boot_id: bootId,
        events,
        next_before: matching.length > size && oldest !== undefined ? oldest.id : null,
      };
    },
    begin: (agents) => {
      entries.length = 0;
      nextId = 1;
      states.clear();
      record({
        at: env.clock.iso(),
        agent: null,
        kind: "hub_started",
        level: "info",
        summary: "Residuum started",
        target: null,
      });
      for (const agent of agents) onState(agent);
    },
    observeHub,
    watchAgent: (agent) => {
      const sendToPages = agent.state.broadcast;
      agent.state.broadcast = (frame) => {
        sendToPages(frame);
        if (frame.type === "session_started") onSessionStarted(agent.name, frame.session);
        else if (frame.type === "session_completed") onSessionCompleted(agent, frame);
      };
    },
    agentReplied: (agent) => {
      record({
        at: env.clock.iso(),
        agent: agent.name,
        kind: "agent_replied",
        level: "info",
        summary: `${agent.name} replied in your conversation`,
        target: chatOf(agent.name),
      });
    },
    userInboxAdded: (agent, itemId) => {
      record({
        at: env.clock.iso(),
        agent: agent.name,
        kind: "inbox_item_added",
        level: "info",
        summary: `${agent.name} added an item to your inbox`,
        target: { kind: "inbox_item", agent: agent.name, item_id: itemId },
      });
    },
  };
}

// ─── Routes ───────────────────────────────────────────────────────────────────

/** An event id from the query. A value that isn't a whole number throws, with the backend's words for it. */
function idParam(name: string, raw: string | null): number | undefined {
  if (raw === null) return undefined;
  if (!/^\d+$/.test(raw)) {
    throw new Error(
      `the ${name} id must be a whole number, not '${raw}' (invalid digit found in string)`,
    );
  }
  return Number(raw);
}

/** The limit from the query. A value that isn't a whole number of at least 1 throws, with the backend's words for it. */
function limitParam(raw: string | null): number | undefined {
  if (raw === null) return undefined;
  if (!/^\d+$/.test(raw)) {
    throw new Error(
      `the limit must be a whole number, not '${raw}' (invalid digit found in string)`,
    );
  }
  if (Number(raw) === 0) throw new Error("the limit must be at least 1");
  return Number(raw);
}

/** `GET /api/hub/events?before=&after=&limit=`: one page of the team event log, newest first. */
function listEvents(ctx: RouteContext): void {
  let query: PageQuery;
  try {
    query = {
      before: idParam("before", ctx.query.get("before")),
      after: idParam("after", ctx.query.get("after")),
      limit: limitParam(ctx.query.get("limit")),
    };
  } catch (err) {
    json(ctx.res, 400, { error: err instanceof Error ? err.message : String(err) });
    return;
  }
  json(ctx.res, 200, ctx.hub.teamEvents.page(query));
}

/** An item's id, as the backend makes it: the day and the title's words, with `_2`, `_3` and so on on a repeat. */
function userInboxItemId(state: MockState, title: string): string {
  const day = state.env.clock.iso().slice(0, 10).replaceAll("-", "");
  const slug = title
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word !== "")
    .join("_")
    .slice(0, 60)
    .replace(/_+$/, "");
  const stem = slug === "" ? day : `${day}_${slug}`;
  const taken = (id: string): boolean =>
    [...state.inboxItems, ...state.inboxArchive].some((item) => item.id === id);
  let id = stem;
  for (let suffix = 2; taken(id); suffix++) id = `${stem}_${String(suffix)}`;
  return id;
}

/**
 * `POST /api/mock/user-inbox-add`, a test control: the agent (`?agent=`, else
 * the first running one) saves an item in the user's inbox the way its
 * `user_inbox_add` tool does. The body is `{ title?, body? }`. Answers `{ id }`.
 */
async function addUserInboxItem(ctx: RouteContext): Promise<void> {
  const agent = ctx.hub.agents.get(ctx.state.agentName);
  if (!agent) {
    json(ctx.res, 404, { error: "mock: name an agent with ?agent=" });
    return;
  }
  const raw = await readBody(ctx.req);
  let body: Record<string, unknown>;
  try {
    body = raw.trim() === "" ? {} : parseJsonObject(raw);
  } catch (err) {
    json(ctx.res, 400, { error: `mock: ${err instanceof Error ? err.message : String(err)}` });
    return;
  }
  const title = stringField(body, "title") ?? `A note from ${agent.name}`;
  const item: UserInboxItem = {
    id: userInboxItemId(agent.state, title),
    title,
    body: stringField(body, "body") ?? "",
    source: "agent",
    timestamp: ctx.hub.env.clock.iso(),
    read: false,
    attachments: [],
  };
  agent.state.inboxItems.unshift(item);
  ctx.hub.teamEvents.userInboxAdded(agent, item.id);
  json(ctx.res, 200, { id: item.id });
}

/** The team event route, in the `/api/hub/...` spelling the hub keeps, and its test control. */
export const teamEventRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/hub/events", handler: listEvents },
  { method: "POST", pattern: "/api/mock/user-inbox-add", handler: addUserInboxItem },
];
