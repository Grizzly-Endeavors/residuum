import type {
  AgentLastError,
  AgentOverview,
  AgentSummary,
  MailThreadSummary,
  OutboundProblem,
  SystemOneOutage,
  SystemOneStatus,
} from "./hub-types";

export const INBOX_THREADS_SHOWN = 5;

export type NeedsYouSeverity = "error" | "warn" | "info";

interface ItemBase {
  key: string;
  severity: NeedsYouSeverity;
  at: string;
}

export interface FailedAgentItem extends ItemBase {
  kind: "failed";
  agent: string;
  error: AgentLastError | null;
}

export interface OutboundProblemItem extends ItemBase {
  kind: "outbound";
  agent: string;
  problem: OutboundProblem;
}

export interface InboxNeedsItem extends ItemBase {
  kind: "inbox";
  thread: MailThreadSummary;
}

export interface SystemOneNeedsItem extends ItemBase {
  kind: "system_one";
  outage: SystemOneOutage;
  provider: string | null;
}

export type NeedsYouItem =
  | FailedAgentItem
  | OutboundProblemItem
  | InboxNeedsItem
  | SystemOneNeedsItem;

export interface NeedsYou {
  items: NeedsYouItem[];
  moreInInbox: number;
  moreInInboxExact: boolean;
  count: number;
}

export interface NeedsYouInput {
  agents: readonly AgentSummary[];
  overviews: Readonly<Record<string, AgentOverview>>;
  unreadThreads: readonly MailThreadSummary[];
  unreadThreadsComplete?: boolean;
  systemOne?: SystemOneStatus | null;
}

const SEVERITY_RANK: Readonly<Record<NeedsYouSeverity, number>> = { error: 0, warn: 1, info: 2 };

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

export function deriveNeedsYou({
  agents,
  overviews,
  unreadThreads,
  unreadThreadsComplete = false,
  systemOne,
}: NeedsYouInput): NeedsYou {
  const items: NeedsYouItem[] = [];
  let inboxUnread = 0;

  if (systemOne?.outage) {
    items.push({
      kind: "system_one",
      key: "system_one",
      severity: "warn",
      at: systemOne.outage.since,
      outage: systemOne.outage,
      provider: systemOne.provider,
    });
  }

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
  const unread = unreadThreads.filter(
    (thread) => thread.unread && thread.participants.some((name) => known.has(name)),
  );
  const inbox = unread.slice(0, INBOX_THREADS_SHOWN);
  for (const thread of inbox) {
    items.push({
      kind: "inbox",
      key: `inbox:${thread.thread_id}`,
      severity: "info",
      at: thread.latest_at,
      thread,
    });
  }

  items.sort(byNeed);
  const listedProblems = items.length - inbox.length;
  const everyThreadSeen = unreadThreadsComplete || inboxUnread <= unread.length;
  const beyondListed = unread.length - inbox.length;
  return {
    items,
    moreInInbox: everyThreadSeen ? beyondListed : Math.max(1, beyondListed),
    moreInInboxExact: everyThreadSeen,
    count:
      listedProblems +
      (everyThreadSeen ? inbox.length : Math.min(INBOX_THREADS_SHOWN, inboxUnread)),
  };
}
