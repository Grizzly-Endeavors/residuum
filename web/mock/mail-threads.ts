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

/**
 * Mail as the hub derives it: no store of its own, only the agents' inboxes.
 * A message an agent sent the user is an item of that agent's user inbox
 * (`inboxItems`, or `inboxArchive` once archived). A message the user sent is
 * a copy in each recipient's own inbox (`agentInbox`) with source `web` or
 * `cli`, all copies sharing one id, which the hub presents as one message.
 * Items of any other source (pulse, action, webhook, artifact) are not mail.
 * A thread is a root message and every message whose `in_reply_to` chain
 * reaches it, and its id is the root's ref.
 */

/** The sources of an agent-inbox item the user wrote. */
const USER_AUTHORED_SOURCES: ReadonlySet<string> = new Set(["web", "cli"]);

/** How many characters of the latest message a thread's snippet holds. */
const SNIPPET_CHARS = 140;

/** An agent's message to the user, and the list it sits in. */
interface AgentMessageHome {
  agent: MockAgent;
  item: UserInboxItem;
  archived: boolean;
}

/** One message of a thread, as the hub presents it. */
export interface MailEntry {
  /** The ref the thread shows the message by; for a message sent to several agents, the first recipient's copy. */
  ref: string;
  /** Every ref that names the message: the copies of a message sent to several agents. */
  refs: string[];
  from: MailSender;
  /** The recipients of a message the user sent; empty for an agent's. */
  to: string[];
  /** When it was written, in milliseconds since the epoch. */
  ms: number;
  at: string;
  title: string;
  body: string;
  attachments: MailAttachment[];
  inReplyTo: string | null;
  read: boolean;
  archived: boolean;
  seenBy: MailSeen[];
  /** The user-inbox item behind a message an agent sent; `null` for the user's. */
  home: AgentMessageHome | null;
}

/** A thread: its id and its messages, the root first and then the replies oldest first. */
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

/** The files of an item, each with the route that serves it and the path the file panel opens it by, which moves with the item into the archive. */
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

/** The copies of one message the user sent, one per recipient and the first recipient's first. */
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

/** Every message in every agent's inboxes, running or not, in no particular order. */
function collectEntries(hub: MockHub): MailEntry[] {
  const agents = [...hub.agents.values()].sort((a, b) => byName(a.name, b.name));
  const entries: MailEntry[] = [];
  const sent = new Map<string, [Copy, ...Copy[]]>();
  for (const agent of agents) {
    for (const item of agent.state.inboxItems) entries.push(agentMessage(agent, item, false));
    for (const item of agent.state.inboxArchive) entries.push(agentMessage(agent, item, true));
    for (const item of agent.state.agentInbox) {
      if (!USER_AUTHORED_SOURCES.has(item.source)) continue;
      // What makes items copies of one message: the same id, words and time.
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

/** A ref taken apart: `<agent>/<user|agent>/<id>`. */
interface ParsedRef {
  agent: string;
  side: InboxSide;
  id: string;
}

/** The parts of a ref, or `null` when `raw` isn't one (the backend's `InboxRef::parse`). */
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

/**
 * The order of refs: by agent, then an agent's messages to you (`user`) before
 * the messages to it (`agent`), then by id. That is not the order of the
 * strings, which puts `agent` before `user`.
 */
export function compareRefs(a: string, b: string): number {
  const left = parseRef(a);
  const right = parseRef(b);
  if (left === null || right === null) return byName(a, b);
  return (
    byName(left.agent, right.agent) || byName(right.side, left.side) || byName(left.id, right.id)
  );
}

/** A reply in its place among the replies, with how far it is from the root. */
interface Reply {
  entry: MailEntry;
  depth: number;
}

/** Replies in time order; within one instant a parent before its replies, then by ref so the order is stable. */
function repliesInOrder(a: Reply, b: Reply): number {
  return a.entry.ms - b.entry.ms || a.depth - b.depth || compareRefs(a.entry.ref, b.entry.ref);
}

/** Every thread, in no particular order. */
export function buildThreads(hub: MockHub): MailThreadData[] {
  const entries = collectEntries(hub);
  const byRef = new Map<string, MailEntry>();
  for (const entry of entries) {
    for (const ref of entry.refs) byRef.set(ref, entry);
  }
  /** The messages from `entry` up to the one its chain of replies starts from; a reply to something that isn't mail starts its own thread. */
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

/** The thread holding the message `ref` names, whichever message of the thread that is. */
export function findThread(hub: MockHub, ref: string): MailThreadData | undefined {
  return buildThreads(hub).find((thread) =>
    thread.entries.some((entry) => entry.refs.includes(ref)),
  );
}

/** The message `ref` names, with the thread it is in. */
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

/** The message a thread starts from, which names it. */
function rootOf(thread: MailThreadData): MailEntry {
  const root = thread.entries[0];
  if (root === undefined) throw new Error(`mail thread '${thread.id}' has no messages`);
  return root;
}

/** What a thread is about: its root message's title. */
export function subjectOf(thread: MailThreadData): string {
  return rootOf(thread).title;
}

/** The message written last: the greatest time, a tie going to the later message. Not always the last one listed, since a reply can carry an earlier time than its root. */
export function latestOf(thread: MailThreadData): MailEntry {
  let latest = rootOf(thread);
  for (const entry of thread.entries) {
    if (entry.ms >= latest.ms) latest = entry;
  }
  return latest;
}

/** The messages of a thread an agent sent the user. */
export function agentMessagesOf(thread: MailThreadData): MailEntry[] {
  return thread.entries.filter((entry) => entry.home !== null);
}

/** Every agent of a thread, as a sender or a recipient, sorted by name. */
export function participantsOf(thread: MailThreadData): string[] {
  const names = new Set<string>();
  for (const entry of thread.entries) {
    const involved = entry.from.kind === "agent" ? [entry.from.agent] : entry.to;
    for (const name of involved) names.add(name);
  }
  return [...names].sort(byName);
}

/** Whether the thread is in `folder`: Inbox has an active message from an agent, Archived only archived ones, Sent a message of the user's. */
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

/** A message's text as one plain line: Markdown markers dropped, whitespace collapsed. */
function plainLine(markdown: string): string {
  return markdown
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|[-*+]|\d+\.|>)\s+/gm, "")
    .replace(/\*\*|__|~~|[*`]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** The start of a message's text, for the list. */
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
    // Every agent that wrote in the thread or was written to in it: a reply goes to all of them.
    recipients_for_reply: participants,
  };
}

/** A reply's title: `Re: <subject>`, with any `Re:` the subject starts with dropped so they don't stack. */
export function replyTitle(subject: string): string {
  let rest = subject.trim();
  while (/^re:/i.test(rest)) rest = rest.slice(3).trimStart();
  return rest === "" ? "Re:" : `Re: ${rest}`;
}
