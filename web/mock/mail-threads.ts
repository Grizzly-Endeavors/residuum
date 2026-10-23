import type {
  MailAttachment,
  MailFolder,
  MailMessage,
  MailSeen,
  MailSender,
  MailThread,
  MailThreadSummary,
} from "../src/lib/hub-types";
import type { UserInboxAttachment, UserInboxItem } from "../src/lib/types";
import { attachmentPath, attachmentUrl, type InboxSide } from "./inbox";
import type { MockAgent, MockAgentInboxItem, MockHub } from "./state";
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
  seconds: number;
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

/** A thread: its id and its messages, oldest first. */
export interface MailThreadData {
  id: string;
  entries: MailEntry[];
}

/** An item's time as seconds since the epoch and as RFC 3339, the way the backend reports it. */
function instantOf(id: string, timestamp: string): { seconds: number; at: string } {
  const ms = Date.parse(timestamp);
  if (Number.isNaN(ms)) {
    throw new Error(`inbox item '${id}' has a time the mock can't read: ${timestamp}`);
  }
  return {
    seconds: Math.floor(ms / 1000),
    at: new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z"),
  };
}

function attachmentsOf(
  agent: string,
  side: InboxSide,
  id: string,
  attachments: readonly UserInboxAttachment[],
): MailAttachment[] {
  return attachments.map((attachment, index) => ({
    filename: attachment.filename,
    size: attachment.size,
    content_type: attachment.mime_type,
    url: attachmentUrl(agent, id, index),
    agent,
    path: attachmentPath(side, id, attachment.filename),
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
    attachments: attachmentsOf(agent.name, "user", item.id, item.attachments),
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
      const key = `${item.id}\n${item.timestamp}`;
      const copy = { agent, item };
      const copies = sent.get(key);
      if (copies === undefined) sent.set(key, [copy]);
      else copies.push(copy);
    }
  }
  for (const copies of sent.values()) entries.push(userMessage(copies));
  return entries;
}

function oldestFirst(a: MailEntry, b: MailEntry): number {
  return a.seconds - b.seconds || byName(a.ref, b.ref);
}

/** Every thread, in no particular order. */
export function buildThreads(hub: MockHub): MailThreadData[] {
  const entries = collectEntries(hub);
  const byRef = new Map<string, MailEntry>();
  for (const entry of entries) {
    for (const ref of entry.refs) byRef.set(ref, entry);
  }
  /** The message a chain of replies starts from; a reply to something that isn't mail starts its own thread. */
  const rootOf = (entry: MailEntry): MailEntry => {
    const visited = new Set<MailEntry>([entry]);
    let current = entry;
    for (;;) {
      const parent = current.inReplyTo === null ? undefined : byRef.get(current.inReplyTo);
      if (parent === undefined || visited.has(parent)) return current;
      visited.add(parent);
      current = parent;
    }
  };
  const threads = new Map<MailEntry, MailEntry[]>();
  for (const entry of entries) {
    const root = rootOf(entry);
    const members = threads.get(root);
    if (members === undefined) threads.set(root, [entry]);
    else members.push(entry);
  }
  return [...threads].map(([root, members]) => ({
    id: root.ref,
    entries: members.sort(oldestFirst),
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

/** What a thread is about: its first message's title. */
export function subjectOf(thread: MailThreadData): string {
  return thread.entries[0]?.title ?? "";
}

/** The messages of a thread an agent sent the user. */
export function agentMessagesOf(thread: MailThreadData): MailEntry[] {
  return thread.entries.filter((entry) => entry.home !== null);
}

/** The agents of a thread in the order they first show up, as senders or recipients. */
export function participantsOf(thread: MailThreadData): string[] {
  const names: string[] = [];
  for (const entry of thread.entries) {
    const involved = entry.from.kind === "agent" ? [entry.from.agent] : entry.to;
    for (const name of involved) {
      if (!names.includes(name)) names.push(name);
    }
  }
  return names;
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
  const latest = thread.entries.at(-1);
  if (latest === undefined) throw new Error(`mail thread '${thread.id}' has no messages`);
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

/** A reply's title: `Re: <subject>`, with no second `Re:` on a subject that has one. */
export function replyTitle(subject: string): string {
  if (/^re:/i.test(subject.trim())) return subject.trim();
  return `Re: ${subject.trim()}`.trimEnd();
}
