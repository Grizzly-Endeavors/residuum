import type { HubInboxItem } from "../src/lib/generated/HubInboxItem";
import type { HubInboxPage } from "../src/lib/generated/HubInboxPage";
import type { HubInboxUnread } from "../src/lib/generated/HubInboxUnread";
import type { InboxStatus } from "../src/lib/generated/InboxStatus";
import type { UserInboxItem } from "../src/lib/types";
import { json } from "./http";
import { toApiInboxItem } from "./inbox";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockAgent, MockState } from "./state";

/** How many items a page holds when the request names no limit. */
const DEFAULT_PAGE_SIZE = 50;

/** The most items a page holds; a larger limit is treated as this. */
const MAX_PAGE_SIZE = 200;

/** The items of one list of an agent's user inbox. */
function itemsOf(state: MockState, status: InboxStatus): UserInboxItem[] {
  return status === "active" ? state.inboxItems : state.inboxArchive;
}

/** An item's time as seconds since the epoch and as RFC 3339, the way the backend reports it. */
function instantOf(item: UserInboxItem): { seconds: number; at: string } {
  const ms = Date.parse(item.timestamp);
  if (Number.isNaN(ms)) {
    throw new Error(`inbox item '${item.id}' has a time the mock can't read: ${item.timestamp}`);
  }
  return {
    seconds: Math.floor(ms / 1000),
    at: new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z"),
  };
}

/**
 * An item as the hub lists it. Its attachments point at the owning agent's
 * routes, as the per-agent inbox lists them.
 */
function hubItem(agent: MockAgent, item: UserInboxItem): HubInboxItem {
  return {
    agent: agent.name,
    id: item.id,
    title: item.title,
    body: item.body,
    source: item.source,
    at: instantOf(item).at,
    read: item.read,
    attachments: toApiInboxItem(agent.state, item).attachments,
  };
}

/** Where a page stopped: the sort key of its last item, as `<unix seconds>:<agent>:<id>`. */
interface Cursor {
  seconds: number;
  agent: string;
  id: string;
}

function renderCursor(cursor: Cursor): string {
  return `${cursor.seconds}:${cursor.agent}:${cursor.id}`;
}

/** The cursor `raw` renders, or `null` when this inbox didn't issue it. */
function parseCursor(raw: string): Cursor | null {
  const match = /^(-?\d+):([^:]+):(.+)$/.exec(raw);
  const [, seconds, agent, id] = match ?? [];
  if (seconds === undefined || agent === undefined || id === undefined) return null;
  return { seconds: Number(seconds), agent, id };
}

/** Descending order: by time, then id, then agent, the backend's total order. */
function compareKeys(a: Cursor, b: Cursor): number {
  if (a.seconds !== b.seconds) return a.seconds < b.seconds ? 1 : -1;
  if (a.id !== b.id) return a.id < b.id ? 1 : -1;
  if (a.agent !== b.agent) return a.agent < b.agent ? 1 : -1;
  return 0;
}

interface Listed {
  key: Cursor;
  item: HubInboxItem;
}

function badRequest(ctx: RouteContext, message: string): void {
  json(ctx.res, 400, { error: message });
}

/** The agent named `name`, or `null` after answering `404`. */
function agentNamed(ctx: RouteContext, name: string): MockAgent | null {
  const agent = ctx.hub.agents.get(name);
  if (!agent) json(ctx.res, 404, { error: `no agent named '${name}'` });
  return agent ?? null;
}

/** `GET /api/hub/inbox`: one page of every agent's items, newest first. */
function listInbox(ctx: RouteContext): void {
  const { query, hub } = ctx;

  const statusParam = query.get("status") ?? "active";
  if (statusParam !== "active" && statusParam !== "archived") {
    badRequest(ctx, `the status must be 'active' or 'archived', not '${statusParam}'`);
    return;
  }
  const status: InboxStatus = statusParam;

  const limitParam = query.get("limit");
  let limit = DEFAULT_PAGE_SIZE;
  if (limitParam !== null) {
    if (!/^\d+$/.test(limitParam)) {
      badRequest(
        ctx,
        `the limit must be a whole number, not '${limitParam}' (invalid digit found in string)`,
      );
      return;
    }
    if (Number(limitParam) === 0) {
      badRequest(ctx, "the limit must be at least 1");
      return;
    }
    limit = Math.min(Number(limitParam), MAX_PAGE_SIZE);
  }

  const beforeParam = query.get("before");
  const before = beforeParam === null ? null : parseCursor(beforeParam);
  if (beforeParam !== null && before === null) {
    badRequest(ctx, "the before cursor isn't one this inbox returned");
    return;
  }

  const agentParam = query.get("agent");
  let agents: MockAgent[];
  if (agentParam === null) {
    agents = [...hub.agents.values()];
  } else {
    const agent = agentNamed(ctx, agentParam);
    if (agent === null) return;
    agents = [agent];
  }

  const listed: Listed[] = agents.flatMap((agent) =>
    itemsOf(agent.state, status).map((item) => ({
      key: { seconds: instantOf(item).seconds, agent: agent.name, id: item.id },
      item: hubItem(agent, item),
    })),
  );
  listed.sort((a, b) => compareKeys(a.key, b.key));
  const after = before === null ? listed : listed.filter((l) => compareKeys(l.key, before) > 0);
  const page = after.slice(0, limit);
  const last = page.at(-1);
  json(ctx.res, 200, {
    items: page.map((l) => l.item),
    next_cursor: after.length > limit && last !== undefined ? renderCursor(last.key) : null,
  } satisfies HubInboxPage);
}

/** `GET /api/hub/inbox/unread`: unread active items, in total and per agent, agents with none included. */
function unreadInbox(ctx: RouteContext): void {
  const byAgent: Record<string, number> = {};
  let total = 0;
  for (const agent of ctx.hub.agents.values()) {
    const unread = agent.state.inboxItems.filter((item) => !item.read).length;
    byAgent[agent.name] = unread;
    total += unread;
  }
  json(ctx.res, 200, { total, by_agent: byAgent } satisfies HubInboxUnread);
}

/** An id names one item, so it can't point elsewhere: the backend's `validate_id`. */
function isBareItemId(id: string): boolean {
  return id !== "" && id !== "." && id !== ".." && !/[/\\\0]/.test(id);
}

/** What a per-item call is about, resolved from the route's two captures. */
interface Target {
  agent: MockAgent;
  id: string;
}

/** The agent and id the route names, or `null` after answering `404` or `400`. */
function targetOf(ctx: RouteContext): Target | null {
  const agent = agentNamed(ctx, decodedParam(ctx, 0));
  if (agent === null) return null;
  const id = decodedParam(ctx, 1);
  if (!isBareItemId(id)) {
    badRequest(ctx, `'${id}' isn't an inbox item id`);
    return null;
  }
  return { agent, id };
}

function respondWithItem(ctx: RouteContext, agent: MockAgent, item: UserInboxItem): void {
  json(ctx.res, 200, { item: hubItem(agent, item) });
}

/** `PUT /api/hub/inbox/{agent}/{id}/read`: mark the item read, in the active inbox or else the archive. */
function readItem(ctx: RouteContext): void {
  const target = targetOf(ctx);
  if (target === null) return;
  const { agent, id } = target;
  const item =
    agent.state.inboxItems.find((i) => i.id === id) ??
    agent.state.inboxArchive.find((i) => i.id === id);
  if (item === undefined) {
    json(ctx.res, 404, { error: `${agent.name} has no inbox item '${id}'` });
    return;
  }
  item.read = true;
  respondWithItem(ctx, agent, item);
}

/** `POST /api/hub/inbox/{agent}/{id}/(archive|restore)`: move an item to the other list. */
function moveItem(ctx: RouteContext, from: InboxStatus): void {
  const target = targetOf(ctx);
  if (target === null) return;
  const { agent, id } = target;
  const to: InboxStatus = from === "active" ? "archived" : "active";
  const toName = to === "active" ? "inbox" : "archive";

  const item = itemsOf(agent.state, from).find((i) => i.id === id);
  if (item === undefined) {
    json(ctx.res, 404, { error: `${agent.name} has no ${from} inbox item '${id}'` });
    return;
  }
  if (itemsOf(agent.state, to).some((i) => i.id === id)) {
    json(ctx.res, 409, {
      error: `the ${toName} already holds a different item with id '${id}' for ${agent.name}, so this one was left where it is`,
    });
    return;
  }

  const source = itemsOf(agent.state, from);
  source.splice(source.indexOf(item), 1);
  itemsOf(agent.state, to).push(item);
  respondWithItem(ctx, agent, item);
}

const ITEM_ROUTE = "^/api/hub/inbox/([^/]+)/([^/]+)";

/** The cross-agent inbox routes (`/api/hub/inbox...`), over each agent's own inbox in the mock state. */
export const hubInboxRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/hub/inbox", handler: listInbox },
  { method: "GET", pattern: "/api/hub/inbox/unread", handler: unreadInbox },
  { method: "PUT", pattern: new RegExp(`${ITEM_ROUTE}/read$`), handler: readItem },
  {
    method: "POST",
    pattern: new RegExp(`${ITEM_ROUTE}/archive$`),
    handler: (ctx) => {
      moveItem(ctx, "active");
    },
  },
  {
    method: "POST",
    pattern: new RegExp(`${ITEM_ROUTE}/restore$`),
    handler: (ctx) => {
      moveItem(ctx, "archived");
    },
  },
];
