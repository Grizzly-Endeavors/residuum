import type {
  MailAttachment,
  MailFolder,
  MailMessage,
  MailSeen,
  MailSender,
  MailThread,
  MailThreadSummary,
} from "../src/lib/hub-types";
import { attachmentPath, attachmentUrl, type InboxSide } from "./inbox";
import type {
  MockAgent,
  MockAgentInboxItem,
  MockHub,
  UserInboxAttachment,
  UserInboxItem,
} from "./state";
import { byName } from "./util";

const USER_AUTHORED_SOURCES: ReadonlySet<string> = new Set(["web", "cli"]);

const SNIPPET_CHARS = 140;

interface AgentMessageHome {
  agent: MockAgent;
  item: UserInboxItem;
  archived: boolean;
}

export interface MailEntry {
  ref: string;
  refs: string[];
  from: MailSender;
  to: string[];
  ms: number;
  at: string;
  title: string;
  body: string;
  attachments: MailAttachment[];
  inReplyTo: string | null;
  read: boolean;
  archived: boolean;
  seenBy: MailSeen[];
  home: AgentMessageHome | null;
}

export interface MailThreadData {
  id: string;
  entries: MailEntry[];
}

/** An item's time as milliseconds since the epoch and as RFC 3339, the way the backend reports it. */
function instantOf(id: string, timestamp: string): { ms: number; at: string } {
  const ms = Date.parse(timestamp);
  if (Number.isNaN(ms)) {
    throw new Error(`inbox item '${id}' has a time the mock can't read: ${timestamp}`);
  }
  return {
    ms,
    at: new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z"),
  };
}

function attachmentsOf(
  agent: string,
  side: InboxSide,
  id: string,
  attachments: readonly UserInboxAttachment[],
  archived: boolean,
): MailAttachment[] {
  return attachments.map((attachment, index) => ({
    filename: attachment.filename,
    size: attachment.size,
    content_type: attachment.mime_type,
    url: attachmentUrl(agent, side, id, index),
    agent,
    path: attachmentPath(side, id, attachment.filename, archived),
  }));
}

function agentMessage(agent: MockAgent, item: UserInboxItem, archived: boolean): MailEntry {
  const ref = `${agent.name}/user/${item.id}`;
  return {
    ref,
    refs: [ref],
    from: { kind: "agent", agent: agent.name },
    to: [],
    ...instantOf(item.id, item.timestamp),
    title: item.title,
    body: item.body,
    attachments: attachmentsOf(agent.name, "user", item.id, item.attachments, archived),
    inReplyTo: item.in_reply_to ?? null,
    read: item.read,
    archived,
    seenBy: [],
    home: { agent, item, archived },
  };
}

interface Copy {
  agent: MockAgent;
  item: MockAgentInboxItem;
}

function userMessage(copies: readonly [Copy, ...Copy[]]): MailEntry {
  const [first] = copies;
  const refs = copies.map((copy) => `${copy.agent.name}/agent/${copy.item.id}`);
  return {
    ref: `${first.agent.name}/agent/${first.item.id}`,
    refs,
    from: { kind: "user" },
    to: copies.map((copy) => copy.agent.name),
    ...instantOf(first.item.id, first.item.timestamp),
    title: first.item.title,
    body: first.item.body,
    attachments: attachmentsOf(
      first.agent.name,
      "agent",
      first.item.id,
      first.item.attachments ?? [],
      false,
    ),
    inReplyTo: first.item.in_reply_to ?? null,
    read: true,
    archived: false,
    seenBy: copies.map((copy) => ({
      agent: copy.agent.name,
      seen: copy.item.read === true || copy.item.archived === true,
    })),
    home: null,
  };
}

function collectEntries(hub: MockHub): MailEntry[] {
  const agents = [...hub.agents.values()].sort((a, b) => byName(a.name, b.name));
  const entries: MailEntry[] = [];
  const sent = new Map<string, [Copy, ...Copy[]]>();
  for (const agent of agents) {
    for (const item of agent.state.inboxItems) entries.push(agentMessage(agent, item, false));
    for (const item of agent.state.inboxArchive) entries.push(agentMessage(agent, item, true));
    for (const item of agent.state.agentInbox) {
      if (!USER_AUTHORED_SOURCES.has(item.source)) continue;
      const key = JSON.stringify([
        item.id,
        item.title,
        item.body,
        instantOf(item.id, item.timestamp).ms,
      ]);
      const copy = { agent, item };
      const copies = sent.get(key);
      if (copies === undefined) sent.set(key, [copy]);
      else copies.push(copy);
    }
  }
  for (const copies of sent.values()) entries.push(userMessage(copies));
  return entries;
}

interface ParsedRef {
  agent: string;
  side: InboxSide;
  id: string;
}

export function parseRef(raw: string): ParsedRef | null {
  const parts = raw.split("/");
  const [agent, side, id] = parts;
  if (parts.length !== 3 || agent === undefined || side === undefined || id === undefined) {
    return null;
  }
  if (side !== "user" && side !== "agent") return null;
  if (agent === "" || id === "" || id.includes("..") || /[\\\0]/.test(id)) return null;
  return { agent, side, id };
}

export function compareRefs(a: string, b: string): number {
  const left = parseRef(a);
  const right = parseRef(b);
  if (left === null || right === null) return byName(a, b);
  return (
    byName(left.agent, right.agent) || byName(right.side, left.side) || byName(left.id, right.id)
  );
}

interface Reply {
  entry: MailEntry;
  depth: number;
}

function repliesInOrder(a: Reply, b: Reply): number {
  return a.entry.ms - b.entry.ms || a.depth - b.depth || compareRefs(a.entry.ref, b.entry.ref);
}

export function buildThreads(hub: MockHub): MailThreadData[] {
  const entries = collectEntries(hub);
  const byRef = new Map<string, MailEntry>();
  for (const entry of entries) {
    for (const ref of entry.refs) byRef.set(ref, entry);
  }
  const chainOf = (entry: MailEntry): [MailEntry, ...MailEntry[]] => {
    const chain: [MailEntry, ...MailEntry[]] = [entry];
    for (let current = entry; current.inReplyTo !== null; ) {
      const parent = byRef.get(current.inReplyTo);
      if (parent === undefined || chain.includes(parent)) break;
      chain.push(parent);
      current = parent;
    }
    return chain;
  };
  const threads = new Map<MailEntry, Reply[]>();
  for (const entry of entries) {
    const chain = chainOf(entry);
    const root = chain[chain.length - 1] ?? entry;
    const replies = threads.get(root) ?? [];
    threads.set(root, replies);
    if (root !== entry) replies.push({ entry, depth: chain.length - 1 });
  }
  return [...threads].map(([root, replies]) => ({
    id: root.ref,
    entries: [root, ...replies.sort(repliesInOrder).map((reply) => reply.entry)],
  }));
}

export function findThread(hub: MockHub, ref: string): MailThreadData | undefined {
  return buildThreads(hub).find((thread) =>
    thread.entries.some((entry) => entry.refs.includes(ref)),
  );
}

export function findEntry(
  hub: MockHub,
  ref: string,
): { thread: MailThreadData; entry: MailEntry } | undefined {
  for (const thread of buildThreads(hub)) {
    const entry = thread.entries.find((candidate) => candidate.refs.includes(ref));
    if (entry !== undefined) return { thread, entry };
  }
  return undefined;
}

function rootOf(thread: MailThreadData): MailEntry {
  const root = thread.entries[0];
  if (root === undefined) throw new Error(`mail thread '${thread.id}' has no messages`);
  return root;
}

export function subjectOf(thread: MailThreadData): string {
  return rootOf(thread).title;
}

export function latestOf(thread: MailThreadData): MailEntry {
  let latest = rootOf(thread);
  for (const entry of thread.entries) {
    if (entry.ms >= latest.ms) latest = entry;
  }
  return latest;
}

export function agentMessagesOf(thread: MailThreadData): MailEntry[] {
  return thread.entries.filter((entry) => entry.home !== null);
}

export function participantsOf(thread: MailThreadData): string[] {
  const names = new Set<string>();
  for (const entry of thread.entries) {
    const involved = entry.from.kind === "agent" ? [entry.from.agent] : entry.to;
    for (const name of involved) names.add(name);
  }
  return [...names].sort(byName);
}

export function isInFolder(thread: MailThreadData, folder: MailFolder): boolean {
  const fromAgents = agentMessagesOf(thread);
  switch (folder) {
    case "inbox":
      return fromAgents.some((entry) => !entry.archived);
    case "archived":
      return fromAgents.length > 0 && fromAgents.every((entry) => entry.archived);
    case "sent":
      return thread.entries.some((entry) => entry.from.kind === "user");
  }
}

function plainLine(markdown: string): string {
  return markdown
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|[-*+]|\d+\.|>)\s+/gm, "")
    .replace(/\*\*|__|~~|[*`]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

export function snippetOf(body: string): string {
  return Array.from(plainLine(body)).slice(0, SNIPPET_CHARS).join("").trimEnd();
}

export function summaryOf(thread: MailThreadData): MailThreadSummary {
  const latest = latestOf(thread);
  return {
    thread_id: thread.id,
    subject: subjectOf(thread),
    participants: participantsOf(thread),
    message_count: thread.entries.length,
    latest_at: latest.at,
    snippet: snippetOf(latest.body),
    unread: agentMessagesOf(thread).some((entry) => !entry.archived && !entry.read),
    has_attachments: thread.entries.some((entry) => entry.attachments.length > 0),
    in_inbox: isInFolder(thread, "inbox"),
    in_sent: isInFolder(thread, "sent"),
    archived: isInFolder(thread, "archived"),
  };
}

export function messageOf(entry: MailEntry): MailMessage {
  return {
    ref: entry.ref,
    from: entry.from,
    to: entry.to,
    at: entry.at,
    title: entry.title,
    body: entry.body,
    attachments: entry.attachments,
    in_reply_to: entry.inReplyTo,
    read: entry.read,
    archived: entry.archived,
    seen_by: entry.seenBy,
  };
}

export function threadOf(thread: MailThreadData): MailThread {
  const participants = participantsOf(thread);
  return {
    thread_id: thread.id,
    subject: subjectOf(thread),
    participants,
    messages: thread.entries.map(messageOf),
    recipients_for_reply: participants,
  };
}

export function replyTitle(subject: string): string {
  let rest = subject.trim();
  while (/^re:/i.test(rest)) rest = rest.slice(3).trimStart();
  return rest === "" ? "Re:" : `Re: ${rest}`;
}
