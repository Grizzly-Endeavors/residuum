// What needs the user, worked out from the hub's agent list, the overview and
// the newest unread inbox items: Home's "Needs you" list and the rail's Home
// count. Each item lasts exactly as long as the condition behind it.

import type {
  AgentLastError,
  AgentOverview,
  AgentSummary,
  HubInboxItem,
  OutboundProblem,
} from "./hub-types";

/** The most inbox items the list shows; the rest are counted under it. */
export const INBOX_ITEMS_SHOWN = 5;

export type NeedsYouSeverity = "error" | "warn" | "info";

interface ItemBase {
  /** Stable across updates, for keyed lists. */
  key: string;
  severity: NeedsYouSeverity;
  /** When the condition began, RFC 3339. Orders items within a severity. */
  at: string;
}

/** An agent couldn't start. */
export interface FailedAgentItem extends ItemBase {
  kind: "failed";
  agent: string;
  error: AgentLastError | null;
}

/** A running agent has a task out to a remote agent it can't reach. */
export interface OutboundProblemItem extends ItemBase {
  kind: "outbound";
  agent: string;
  problem: OutboundProblem;
}

/** An unread item in an agent's user inbox. */
export interface InboxNeedsItem extends ItemBase {
  kind: "inbox";
  item: HubInboxItem;
}

export type NeedsYouItem = FailedAgentItem | OutboundProblemItem | InboxNeedsItem;

export interface NeedsYou {
  /** Error first, then warn, then info; newest first within each. */
  items: NeedsYouItem[];
  /** Unread inbox items beyond the ones listed. */
  moreInInbox: number;
  /**
   * The rail's Home count: one per item, with the inbox counted up to the
   * five the list shows, whether or not their details have loaded.
   */
  count: number;
}

export interface NeedsYouInput {
  agents: readonly AgentSummary[];
  overviews: Readonly<Record<string, AgentOverview>>;
  /** The newest unread user-inbox items across agents, newest first. */
  unreadItems: readonly HubInboxItem[];
}

const SEVERITY_RANK: Readonly<Record<NeedsYouSeverity, number>> = { error: 0, warn: 1, info: 2 };

/** An item's time for ordering; one with no readable time goes after the dated ones. */
function timeOf(item: NeedsYouItem): number {
  const ms = Date.parse(item.at);
  return Number.isNaN(ms) ? -Infinity : ms;
}

function byNeed(a: NeedsYouItem, b: NeedsYouItem): number {
  const severity = SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity];
  if (severity !== 0) return severity;
  const newer = timeOf(b) - timeOf(a);
  if (newer !== 0 && !Number.isNaN(newer)) return newer;
  if (a.key === b.key) return 0;
  return a.key < b.key ? -1 : 1;
}

export function deriveNeedsYou({ agents, overviews, unreadItems }: NeedsYouInput): NeedsYou {
  const items: NeedsYouItem[] = [];
  let inboxUnread = 0;

  for (const agent of agents) {
    const overview = overviews[agent.name];
    inboxUnread += overview?.inbox_unread ?? 0;
    if (agent.state === "failed") {
      items.push({
        kind: "failed",
        key: `failed:${agent.name}`,
        severity: "error",
        at: agent.last_error?.at ?? "",
        agent: agent.name,
        error: agent.last_error,
      });
    }
    // Only a running agent watches its tasks, and only it can stop them.
    if (agent.state !== "running") continue;
    for (const problem of overview?.outbound_problems ?? []) {
      items.push({
        kind: "outbound",
        key: `outbound:${agent.name}:${problem.task_id}`,
        severity: "warn",
        at: problem.unreachable_since,
        agent: agent.name,
        problem,
      });
    }
  }

  const known = new Set(agents.map((agent) => agent.name));
  const inbox = unreadItems
    .filter((item) => !item.read && known.has(item.agent))
    .slice(0, INBOX_ITEMS_SHOWN);
  for (const item of inbox) {
    items.push({
      kind: "inbox",
      key: `inbox:${item.agent}:${item.id}`,
      severity: "info",
      at: item.at,
      item,
    });
  }

  items.sort(byNeed);
  const listedProblems = items.length - inbox.length;
  return {
    items,
    moreInInbox: Math.max(0, inboxUnread - inbox.length),
    count: listedProblems + Math.min(INBOX_ITEMS_SHOWN, inboxUnread),
  };
}
