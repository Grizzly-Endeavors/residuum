import type { MailFolder, MailThreadPage } from "../src/lib/hub-types";
import { agentInboxItemId, deriveTitle } from "./agent-inbox";
import { json, readBodyBytes, readJsonObject, text, type JsonObject } from "./http";
import { moveAttachments, storeAttachment } from "./inbox";
import {
  agentMessagesOf,
  buildThreads,
  compareRefs,
  findEntry,
  findThread,
  isInFolder,
  latestOf,
  messageOf,
  parseRef,
  participantsOf,
  replyTitle,
  subjectOf,
  summaryOf,
  threadOf,
  type MailThreadData,
} from "./mail-threads";
import type { Route, RouteContext } from "./routes";
import type { MockAgent, MockUpload } from "./state";
import { byName } from "./util";

/**
 * The mail routes (`/api/hub/mail/...`) and the upload route
 * (`/api/hub/uploads`), over the agents' inboxes (see `mail-threads.ts`). The
 * hub serves them whatever the agents' state, so they work with every agent
 * stopped. Failures are `{ "error": message }`.
 */

/** The largest file the hub stages, in bytes: `MAX_UPLOAD_BYTES` in `src/lib/api.ts`. */
export const MAX_UPLOAD_BYTES = 50 * 1024 * 1024;

/** How many threads a page holds when the request names no limit. */
const DEFAULT_PAGE_SIZE = 50;

/** The most threads a page holds; a larger limit is treated as this. */
const MAX_PAGE_SIZE = 200;

const FOLDERS: readonly MailFolder[] = ["inbox", "sent", "archived"];

function fail(ctx: RouteContext, status: number, message: string): void {
  json(ctx.res, status, { error: message });
}

function isFolder(value: string): value is MailFolder {
  return FOLDERS.some((folder) => folder === value);
}

// ─── Listing ──────────────────────────────────────────────────────────────────

/** Where a page stopped: the sort key of its last thread, as `<unix milliseconds>:<thread id>`. */
interface Cursor {
  ms: number;
  threadId: string;
}

function renderCursor(cursor: Cursor): string {
  return `${String(cursor.ms)}:${cursor.threadId}`;
}

/** The cursor `raw` renders, or `null` when this inbox didn't issue it. */
function parseCursor(raw: string): Cursor | null {
  const match = /^(-?\d+):(.+)$/.exec(raw);
  const [, ms, threadId] = match ?? [];
  if (ms === undefined || threadId === undefined || parseRef(threadId) === null) return null;
  return { ms: Number(ms), threadId };
}

/** The list's order: the latest message's time, newest first, then the thread's ref in ascending order. */
function compareKeys(a: Cursor, b: Cursor): number {
  if (a.ms !== b.ms) return a.ms < b.ms ? 1 : -1;
  return compareRefs(a.threadId, b.threadId);
}

/** The page size the request asks for, or `null` after answering `400`. */
function requestedLimit(ctx: RouteContext): number | null {
  const param = ctx.query.get("limit");
  if (param === null) return DEFAULT_PAGE_SIZE;
  if (!/^\d+$/.test(param)) {
    fail(
      ctx,
      400,
      `the limit must be a whole number, not '${param}' (invalid digit found in string)`,
    );
    return null;
  }
  if (Number(param) === 0) {
    fail(ctx, 400, "the limit must be at least 1");
    return null;
  }
  return Math.min(Number(param), MAX_PAGE_SIZE);
}

/** `GET /api/hub/mail/threads`: one page of a folder's threads, newest first. */
function listThreads(ctx: RouteContext): void {
  const folderParam = ctx.query.get("folder") ?? "inbox";
  if (!isFolder(folderParam)) {
    fail(ctx, 400, `the folder must be 'inbox', 'sent' or 'archived', not '${folderParam}'`);
    return;
  }
  const agent = ctx.query.get("agent");
  if (agent !== null && !ctx.hub.agents.has(agent)) {
    fail(ctx, 400, `there is no agent named '${agent}'`);
    return;
  }
  const limit = requestedLimit(ctx);
  if (limit === null) return;
  const beforeParam = ctx.query.get("before");
  const before = beforeParam === null ? null : parseCursor(beforeParam);
  if (beforeParam !== null && before === null) {
    fail(ctx, 400, "the before cursor isn't one this list returned");
    return;
  }

  const listed = buildThreads(ctx.hub)
    .filter((thread) => isInFolder(thread, folderParam))
    .filter((thread) => agent === null || participantsOf(thread).includes(agent))
    .map((thread) => {
      const summary = summaryOf(thread);
      return { key: { ms: latestOf(thread).ms, threadId: thread.id }, summary };
    });
  listed.sort((a, b) => compareKeys(a.key, b.key));
  const after = before === null ? listed : listed.filter((row) => compareKeys(row.key, before) > 0);
  const page = after.slice(0, limit);
  const last = page.at(-1);
  json(ctx.res, 200, {
    threads: page.map((row) => row.summary),
    next: after.length > limit && last !== undefined ? renderCursor(last.key) : null,
  } satisfies MailThreadPage);
}

// ─── One thread ───────────────────────────────────────────────────────────────

/**
 * The text of a percent-encoded path segment the way the backend's router
 * reads it: an escape that isn't one is left as it is, and bytes that aren't
 * UTF-8 make the whole segment unreadable (`null`).
 */
function decodeSegment(raw: string): string | null {
  const bytes: number[] = [];
  const encoder = new TextEncoder();
  for (let at = 0; at < raw.length; ) {
    const escape = /^%([0-9A-Fa-f]{2})/.exec(raw.slice(at, at + 3));
    if (escape?.[1] !== undefined) {
      bytes.push(parseInt(escape[1], 16));
      at += 3;
    } else {
      const char = String.fromCodePoint(raw.codePointAt(at) ?? 0);
      bytes.push(...encoder.encode(char));
      at += char.length;
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes));
  } catch {
    return null;
  }
}

/** The answer for a ref that names no message. */
function noMessage(ctx: RouteContext, ref: string): void {
  fail(ctx, 404, `There is no message '${ref}'. It may have been deleted.`);
}

/** The thread the route names, by the ref of any of its messages, or `null` after answering `400` or `404`. */
function requestedThread(ctx: RouteContext): MailThreadData | null {
  const raw = ctx.params[0] ?? "";
  const ref = decodeSegment(raw);
  if (ref === null) {
    // The router refuses a path parameter that isn't UTF-8 before any route sees it.
    text(ctx.res, 400, "Invalid URL: Invalid UTF-8 in `thread`");
    return null;
  }
  const thread = findThread(ctx.hub, ref);
  if (thread === undefined) noMessage(ctx, ref);
  return thread ?? null;
}

/** `GET /api/hub/mail/threads/{thread_id}`: the thread with every message. */
function getThread(ctx: RouteContext): void {
  const thread = requestedThread(ctx);
  if (thread !== null) json(ctx.res, 200, threadOf(thread));
}

/** What a thread action does to the user-inbox items of the thread's agent messages. */
type ThreadAction = "read" | "archive" | "restore";

/**
 * `POST /api/hub/mail/threads/{thread_id}/(read|archive|restore)`: act on the
 * user-inbox items of the thread, and never on the copies in the agents' own
 * inboxes. Opening a thread reads all of them, archiving moves the active ones
 * to the archive, and restoring moves the archived ones back.
 */
function actOnThread(ctx: RouteContext, action: ThreadAction): void {
  const thread = requestedThread(ctx);
  if (thread === null) return;
  const changed = new Set<MockAgent>();
  for (const entry of agentMessagesOf(thread)) {
    const home = entry.home;
    if (home === null) continue;
    const { state } = home.agent;
    if (action === "read") {
      if (home.item.read) continue;
      home.item.read = true;
    } else {
      const from = action === "archive" ? state.inboxItems : state.inboxArchive;
      const to = action === "archive" ? state.inboxArchive : state.inboxItems;
      const at = from.indexOf(home.item);
      if (at === -1) continue;
      from.splice(at, 1);
      to.push(home.item);
      moveAttachments(state, home.item, action === "archive");
    }
    changed.add(home.agent);
  }
  for (const agent of changed) ctx.hub.overview.changed(agent);
  ctx.res.writeHead(204);
  ctx.res.end();
}

// ─── Uploads ──────────────────────────────────────────────────────────────────

/** Why a filename can't name an upload, for the end of "The filename ...", or `null` when it can. */
function filenameProblem(filename: string): string | null {
  if (filename.trim() === "") return "is empty";
  if (/[/\\]/.test(filename)) return "can't contain a / or a \\";
  if (filename === "." || filename === "..") return "isn't a usable name";
  if (filename.includes("\0")) return "can't contain a null character";
  return null;
}

/** `POST /api/hub/uploads?filename=`: stage the body as a file a later message can name. */
async function stageUpload(ctx: RouteContext): Promise<void> {
  const bytes = await readBodyBytes(ctx.req, MAX_UPLOAD_BYTES);
  if (bytes === null) {
    fail(
      ctx,
      413,
      `The file is larger than the ${String(MAX_UPLOAD_BYTES / (1024 * 1024))} MB upload limit.`,
    );
    return;
  }
  const filename = ctx.query.get("filename");
  if (filename === null) {
    fail(ctx, 400, "Name the upload with a filename query parameter, such as ?filename=notes.txt.");
    return;
  }
  const problem = filenameProblem(filename);
  if (problem !== null) {
    fail(ctx, 400, `The filename ${problem}.`);
    return;
  }
  if (bytes.length === 0) {
    fail(ctx, 400, "The upload is empty.");
    return;
  }
  const id = `upload_${String(ctx.hub.env.nextId()).padStart(4, "0")}`;
  ctx.hub.hubState.uploads.set(id, { id, filename, bytes });
  json(ctx.res, 200, { id, filename, size: bytes.length });
}

/** The staged files the ids name, or `null` after answering `400` for an id nothing is staged under. */
function stagedUploads(ctx: RouteContext, ids: readonly string[]): MockUpload[] | null {
  const uploads: MockUpload[] = [];
  for (const id of new Set(ids)) {
    const upload = ctx.hub.hubState.uploads.get(id);
    if (upload === undefined) {
      fail(
        ctx,
        400,
        `An attached file (upload '${id}') isn't available anymore. Attach it again and send.`,
      );
      return null;
    }
    uploads.push(upload);
  }
  return uploads;
}

/** A file name no earlier name in `taken` has: `report.pdf`, then `report (2).pdf`. */
function freeFilename(filename: string, taken: ReadonlySet<string>): string {
  if (!taken.has(filename)) return filename;
  const dot = filename.lastIndexOf(".");
  const stem = dot > 0 ? filename.slice(0, dot) : filename;
  const extension = dot > 0 ? filename.slice(dot) : "";
  for (let n = 2; ; n++) {
    const candidate = `${stem} (${String(n)})${extension}`;
    if (!taken.has(candidate)) return candidate;
  }
}

// ─── Sending ──────────────────────────────────────────────────────────────────

/** Answer `400` for a body the routes can't read, with the reason as the backend words it. */
function unreadable(ctx: RouteContext, reason: string): null {
  fail(ctx, 400, `The message couldn't be read: ${reason}`);
  return null;
}

/** The request's JSON body, or `null` after answering `400`. */
async function requestBody(ctx: RouteContext): Promise<JsonObject | null> {
  try {
    return await readJsonObject(ctx.req);
  } catch (err) {
    const why = err instanceof Error ? err.message : String(err);
    return unreadable(ctx, `Failed to parse the request body as JSON: ${why}`);
  }
}

/** A string field. `fallback` stands in when it is absent; without one it is required. `null` after answering `400`. */
function textField(
  ctx: RouteContext,
  body: JsonObject,
  key: string,
  fallback?: string,
): string | null {
  const value = body[key];
  if (typeof value === "string") return value;
  if (value === undefined && fallback !== undefined) return fallback;
  return unreadable(
    ctx,
    value === undefined
      ? `Failed to deserialize the JSON body into the target type: missing field \`${key}\``
      : `Failed to deserialize the JSON body into the target type: \`${key}\` must be text`,
  );
}

/** A list-of-strings field. `fallback` stands in when it is absent; without one it is required. `null` after answering `400`. */
function listField(
  ctx: RouteContext,
  body: JsonObject,
  key: string,
  fallback?: string[],
): string[] | null {
  const value = body[key];
  if (Array.isArray(value) && value.every((entry): entry is string => typeof entry === "string")) {
    return value;
  }
  if (value === undefined && fallback !== undefined) return fallback;
  return unreadable(
    ctx,
    value === undefined
      ? `Failed to deserialize the JSON body into the target type: missing field \`${key}\``
      : `Failed to deserialize the JSON body into the target type: \`${key}\` must be a list of text`,
  );
}

/**
 * Write a message to each recipient's own inbox as one copy per recipient, all
 * with one id, moving the staged files into the copies' attachments. Answers
 * the first recipient's ref, which names the message.
 */
function deliver(
  ctx: RouteContext,
  recipients: readonly MockAgent[],
  message: { title: string; body: string; inReplyTo?: string },
  uploads: readonly MockUpload[],
): string {
  // Taken in every agent's inbox, so two messages sent in the same instant never share an id.
  const id = agentInboxItemId(
    [...ctx.hub.agents.values()].map((agent) => agent.state),
    message.title,
  );
  const timestamp = ctx.hub.env.clock.iso();
  const taken = new Set<string>();
  const files = uploads.map((upload) => {
    const filename = freeFilename(upload.filename, taken);
    taken.add(filename);
    return { filename, bytes: upload.bytes };
  });
  for (const agent of recipients) {
    agent.state.agentInbox.push({
      id,
      title: message.title,
      body: message.body,
      source: "web",
      timestamp,
      ...(message.inReplyTo === undefined ? {} : { in_reply_to: message.inReplyTo }),
      attachments: files.map((file) =>
        storeAttachment(agent.state, "agent", id, file.filename, file.bytes),
      ),
    });
  }
  for (const upload of uploads) ctx.hub.hubState.uploads.delete(upload.id);
  const [first] = recipients;
  return `${first?.name ?? ""}/agent/${id}`;
}

/**
 * The agents `to` names, each once and in name order, or `null` after
 * answering `400` for one that doesn't exist.
 */
function recipientsOf(ctx: RouteContext, to: readonly string[]): MockAgent[] | null {
  const recipients: MockAgent[] = [];
  for (const name of [...new Set(to)].sort(byName)) {
    const agent = ctx.hub.agents.get(name);
    if (agent === undefined) {
      fail(ctx, 400, `There is no agent named '${name}'.`);
      return null;
    }
    recipients.push(agent);
  }
  return recipients;
}

/** `POST /api/hub/mail/send`: a new message to one or more agents. Answers the thread it starts. */
async function sendMail(ctx: RouteContext): Promise<void> {
  const body = await requestBody(ctx);
  if (body === null) return;
  const to = listField(ctx, body, "to");
  if (to === null) return;
  const subject = textField(ctx, body, "subject", "");
  if (subject === null) return;
  const content = textField(ctx, body, "body");
  if (content === null) return;
  const attachments = listField(ctx, body, "attachments", []);
  if (attachments === null) return;

  const uploads = stagedUploads(ctx, attachments);
  if (uploads === null) return;
  if (to.length === 0) {
    fail(ctx, 400, "Choose at least one agent to send the message to.");
    return;
  }
  if (content.trim() === "") {
    fail(ctx, 400, "Write a message before sending it.");
    return;
  }
  const recipients = recipientsOf(ctx, to);
  if (recipients === null) return;

  const title = subject.trim() === "" ? deriveTitle(content) : subject;
  const ref = deliver(ctx, recipients, { title, body: content }, uploads);
  json(ctx.res, 200, { thread_id: ref });
}

/** `POST /api/hub/mail/threads/{thread_id}/reply`: reply to everyone in the thread. Answers the message added. */
async function replyToThread(ctx: RouteContext): Promise<void> {
  // An unknown thread answers 404 before the body is looked at.
  const thread = requestedThread(ctx);
  if (thread === null) return;
  const body = await requestBody(ctx);
  if (body === null) return;
  const content = textField(ctx, body, "body");
  if (content === null) return;
  const attachments = listField(ctx, body, "attachments", []);
  if (attachments === null) return;

  const uploads = stagedUploads(ctx, attachments);
  if (uploads === null) return;
  if (content.trim() === "") {
    fail(ctx, 400, "Write a message before sending it.");
    return;
  }
  const recipients = recipientsOf(ctx, participantsOf(thread));
  if (recipients === null) return;

  const ref = deliver(
    ctx,
    recipients,
    {
      title: replyTitle(subjectOf(thread)),
      body: content,
      inReplyTo: latestOf(thread).ref,
    },
    uploads,
  );
  const added = findEntry(ctx.hub, ref);
  if (added === undefined) throw new Error(`the reply ${ref} is in no thread`);
  json(ctx.res, 200, { message: messageOf(added.entry) });
}

const THREAD_ROUTE = "^/api/hub/mail/threads/([^/]+)";

/** The mail routes and the upload route, in the `/api/hub/...` spelling the hub keeps. */
export const mailRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/hub/mail/threads", handler: listThreads },
  { method: "GET", pattern: new RegExp(`${THREAD_ROUTE}$`), handler: getThread },
  { method: "POST", pattern: "/api/hub/mail/send", handler: sendMail },
  { method: "POST", pattern: new RegExp(`${THREAD_ROUTE}/reply$`), handler: replyToThread },
  {
    method: "POST",
    pattern: new RegExp(`${THREAD_ROUTE}/read$`),
    handler: (ctx) => {
      actOnThread(ctx, "read");
    },
  },
  {
    method: "POST",
    pattern: new RegExp(`${THREAD_ROUTE}/archive$`),
    handler: (ctx) => {
      actOnThread(ctx, "archive");
    },
  },
  {
    method: "POST",
    pattern: new RegExp(`${THREAD_ROUTE}/restore$`),
    handler: (ctx) => {
      actOnThread(ctx, "restore");
    },
  },
  { method: "POST", pattern: "/api/hub/uploads", handler: stageUpload },
];
