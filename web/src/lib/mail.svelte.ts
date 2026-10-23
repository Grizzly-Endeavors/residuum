// ── Mail (Svelte 5 runes) ────────────────────────────────────────────
//
// The Inbox's mail across every agent, through the hub's mail routes: the
// thread list of one folder and agent filter with its older pages, the open
// thread, reading, archiving and restoring threads, and the drafts (one New
// message, a reply per thread) with their staged attachments. Counts aren't
// kept here: the overview store has them, and the list and the open thread
// are fetched again when they change. Every failure lands in state the place
// shows with Try again; nothing here navigates.

import {
  ApiError,
  archiveMailThread,
  fetchMailThread,
  fetchMailThreads,
  markMailThreadRead,
  MAX_UPLOAD_BYTES,
  replyToMailThread,
  restoreMailThread,
  sendMail,
  uploadFile,
} from "./api";
import { userErrorMessage } from "./errors";
import type { MailFolder, MailMessage, MailThread, MailThreadSummary } from "./hub-types";
import type { AppLocation } from "./routes";

/** Which threads the list shows: one folder, for one agent or every agent. */
export interface MailList {
  folder: MailFolder;
  agent: string | null;
}

/** What can be done to a whole thread. */
export type ThreadAction = "read" | "archive" | "restore";

/** An action on a thread that failed, and why, in plain words. */
export interface ThreadProblem {
  action: ThreadAction;
  message: string;
}

/** Where the open thread stands: nothing asked for, fetching, shown, gone, or not reachable. */
export type OpenThreadStatus = "idle" | "loading" | "ready" | "missing" | "error";

/**
 * How opening a thread went: shown (with the thread's own id, which may differ
 * from the ref asked for), gone, not reachable, or overtaken by another open
 * or a close before it finished.
 */
export type OpenThreadOutcome =
  | { status: "shown"; threadId: string }
  | { status: "missing" }
  | { status: "failed" }
  | { status: "superseded" };

/** Where a thread's messages from agents are: any still in the inbox, all archived, or none from an agent. */
export type ThreadMailbox = "inbox" | "archived" | null;

/** How far a staged attachment has got: uploading, staged on the hub, or failed. */
export type AttachmentStatus = "uploading" | "uploaded" | "failed";

/** How many threads one page asks for. */
const PAGE_SIZE = 50;
/** The most one request may ask for, so a reload of a long-paged list stays one request. */
const MAX_PAGE_SIZE = 200;

const FAILED: Readonly<Record<ThreadAction, string>> = {
  read: "Couldn't mark it read.",
  archive: "Couldn't archive it.",
  restore: "Couldn't move it to the inbox.",
};

const GONE = "That conversation isn't there any more.";

const UPLOAD_LIMIT = `${String(MAX_UPLOAD_BYTES / (1024 * 1024))} MB`;

/** The folder a thread is in when its flags say so. */
function inFolder(summary: MailThreadSummary, folder: MailFolder): boolean {
  switch (folder) {
    case "inbox":
      return summary.in_inbox;
    case "sent":
      return summary.in_sent;
    case "archived":
      return summary.archived;
  }
}

function fromAgent(message: MailMessage): boolean {
  return message.from.kind === "agent";
}

/** Where `thread`'s agent messages are, which says whether it can be archived or moved to the inbox. */
export function mailboxOf(thread: MailThread): ThreadMailbox {
  const agents = thread.messages.filter(fromAgent);
  if (agents.length === 0) return null;
  return agents.some((message) => !message.archived) ? "inbox" : "archived";
}

/** A message whose ref names a message of `thread`, or the thread itself. */
function holdsRef(thread: MailThread, ref: string): boolean {
  return thread.thread_id === ref || thread.messages.some((message) => message.ref === ref);
}

function sameList(a: MailList, b: MailList): boolean {
  return a.folder === b.folder && a.agent === b.agent;
}

function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) return record;
  return Object.fromEntries(Object.entries(record).filter(([k]) => k !== key));
}

function listing(names: readonly string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1] ?? ""}`;
}

// ── Attachments and drafts ───────────────────────────────────────────

let attachmentCount = 0;

/**
 * A file picked for a message. Its upload starts as soon as it is picked; the
 * message is sent with the hub's id for it.
 */
export class StagedAttachment {
  /** Stable for keyed lists while the draft holds it. */
  readonly key = `attachment-${String(++attachmentCount)}`;
  readonly filename: string;
  readonly size: number;
  status = $state<AttachmentStatus>("uploading");
  /** The hub's id for the staged file, once uploaded. */
  uploadId = $state<string | null>(null);
  /** Why the upload failed, in plain words, or null. */
  error = $state<string | null>(null);
  /** Settles when the current upload attempt does, whichever way it goes. */
  done: Promise<void> = Promise.resolve();
  /** Taken off the draft: a late upload answer changes nothing. */
  private removed = false;
  /** Cancels the upload attempt in flight. */
  private controller: AbortController | null = null;

  constructor(readonly file: File) {
    this.filename = file.name;
    this.size = file.size;
  }

  /** Upload the file, or upload it again after a failure. */
  upload(): Promise<void> {
    if (this.status === "uploaded") return this.done;
    this.status = "uploading";
    this.error = null;
    const controller = new AbortController();
    this.controller = controller;
    this.done = (async () => {
      try {
        const staged = await uploadFile(this.file, this.filename, { signal: controller.signal });
        if (this.removed) return;
        this.uploadId = staged.id;
        this.status = "uploaded";
      } catch (err) {
        if (this.removed) return;
        this.error = userErrorMessage(err, { action: `Couldn't upload ${this.filename}.` });
        this.status = "failed";
      }
    })();
    return this.done;
  }

  /** Take it off the draft: an upload still running is cancelled, and nothing it answers later counts. */
  remove(): void {
    this.removed = true;
    this.controller?.abort();
  }
}

/** What a message being written holds: its body and its attachments. */
export class MailDraft {
  body = $state("");
  attachments = $state<StagedAttachment[]>([]);
  /** Files that weren't attached because they are too large, said in plain words, or null. */
  attachNotice = $state<string | null>(null);
  sending = $state(false);
  /** Why the last send didn't go, in plain words, or null. */
  sendError = $state<string | null>(null);

  /** The draft holds something the user wrote or attached. */
  get dirty(): boolean {
    return this.body.trim() !== "" || this.attachments.length > 0;
  }

  /** Some attachment is still uploading. */
  get uploading(): boolean {
    return this.attachments.some((attachment) => attachment.status === "uploading");
  }

  /**
   * Attach `files`: each starts uploading at once. A file over the upload
   * limit is refused before any upload, and `attachNotice` names it.
   */
  attach(files: Iterable<File>): void {
    const refused: string[] = [];
    const added: StagedAttachment[] = [];
    for (const file of files) {
      if (file.size > MAX_UPLOAD_BYTES) {
        refused.push(file.name);
        continue;
      }
      const attachment = new StagedAttachment(file);
      void attachment.upload();
      added.push(attachment);
    }
    this.attachNotice =
      refused.length === 0
        ? null
        : `${listing(refused)} ${refused.length === 1 ? "is" : "are"} larger than the ${UPLOAD_LIMIT} upload limit, so ${refused.length === 1 ? "it wasn't" : "they weren't"} attached.`;
    if (added.length > 0) this.attachments = [...this.attachments, ...added];
  }

  /** Upload a failed attachment again. */
  retryAttachment(key: string): Promise<void> {
    const attachment = this.attachments.find((held) => held.key === key);
    return attachment?.status === "failed" ? attachment.upload() : Promise.resolve();
  }

  /** Take an attachment off the message, uploaded or not. */
  removeAttachment(key: string): void {
    const attachment = this.attachments.find((held) => held.key === key);
    if (attachment === undefined) return;
    attachment.remove();
    this.attachments = this.attachments.filter((held) => held !== attachment);
  }

  dismissAttachNotice(): void {
    this.attachNotice = null;
  }

  /** Why the draft can't be sent as it stands, in plain words, or null when it can. */
  get invalid(): string | null {
    return this.body.trim() === "" ? "Write a message before sending it." : null;
  }

  /**
   * Wait for the uploads still running, then the ids of every attachment, or
   * null with `sendError` set when one didn't upload.
   */
  async uploadIds(): Promise<string[] | null> {
    await Promise.all(this.attachments.map((attachment) => attachment.done));
    const failed = this.attachments.filter((attachment) => attachment.uploadId === null);
    if (failed.length > 0) {
      const names = listing(failed.map((attachment) => attachment.filename));
      this.sendError = `${names} didn't upload. Try ${failed.length === 1 ? "it" : "them"} again or remove ${failed.length === 1 ? "it" : "them"}, then send.`;
      return null;
    }
    return this.attachments.map((attachment) => attachment.uploadId ?? "");
  }

  /** Empty the draft after it was sent. */
  clear(): void {
    for (const attachment of this.attachments) attachment.remove();
    this.body = "";
    this.attachments = [];
    this.attachNotice = null;
    this.sendError = null;
  }
}

/** A reply to everyone in a thread. */
export class ReplyDraft extends MailDraft {
  constructor(readonly threadId: string) {
    super();
  }

  override get invalid(): string | null {
    return this.body.trim() === "" ? "Write a reply before sending it." : null;
  }
}

/** A New message: its recipients and subject as well as its body. */
export class NewMessageDraft extends MailDraft {
  to = $state<string[]>([]);
  /** May stay empty: the thread then shows as "(no subject)". */
  subject = $state("");

  constructor(to: readonly string[] = []) {
    super();
    this.to = [...to];
  }

  /** Recipients alone, as a link prefills them, are not something the user wrote. */
  override get dirty(): boolean {
    return super.dirty || this.subject.trim() !== "";
  }

  override get invalid(): string | null {
    if (this.to.length === 0) return "Add at least one agent to send it to.";
    return super.invalid;
  }
}

// ── The store ────────────────────────────────────────────────────────

export class MailStore {
  // The list

  /** The list being shown, or null before the Inbox first opens. */
  list = $state.raw<MailList | null>(null);
  /** The list's threads, newest first, as far as it has been paged. */
  threads = $state<MailThreadSummary[]>([]);
  /** Where the next older page starts, or null when the list ends there. */
  next = $state<string | null>(null);
  /** The list's first page has arrived. */
  loaded = $state(false);
  /** Why the list couldn't be fetched, or null. */
  loadError = $state<string | null>(null);
  loadingMore = $state(false);
  /** Why an older page couldn't be fetched, or null. */
  moreError = $state<string | null>(null);

  // The open thread

  /** The ref the open thread was asked for by, or null when none is open. */
  openRef = $state<string | null>(null);
  /** The open thread, once fetched. */
  thread = $state<MailThread | null>(null);
  threadStatus = $state<OpenThreadStatus>("idle");
  /** Why the open thread couldn't be fetched, or null. */
  threadError = $state<string | null>(null);

  // Thread actions

  /** The archive or restore in flight on each thread, by thread id. */
  pending = $state<Record<string, "archive" | "restore">>({});
  /** The last failed action on each thread, by thread id, until it is tried again. */
  problems = $state<Record<string, ThreadProblem>>({});

  // Drafts

  /** The New message being written, or null when none is open. */
  newMessage = $state<NewMessageDraft | null>(null);
  /** The reply being written in each thread, by thread id; a thread gets one when it opens. */
  replies = $state<Record<string, ReplyDraft>>({});

  private listRequest = 0;
  private openRequest = 0;
  /** Threads whose read is in flight, so a second open doesn't ask again. */
  private reading: string[] = [];
  /** The unread counts the list was last fetched for; null while the Inbox isn't open. */
  private countsSeen: string | null = null;

  // ── The list ───────────────────────────────────────────────────────

  /**
   * Show `list` and fetch it. The threads already held stay on screen while
   * the same list is fetched again; another list starts empty.
   */
  show(list: MailList): Promise<void> {
    if (this.list === null || !sameList(this.list, list)) {
      this.list = list;
      this.threads = [];
      this.next = null;
      this.loaded = false;
      this.loadError = null;
    }
    this.moreError = null;
    return this.reload();
  }

  /** The Inbox closed: stop following the counts until it opens again. */
  leave(): void {
    this.countsSeen = null;
  }

  /**
   * The unread counts as the overview has them (`overview.unreadCounts()`):
   * when they change, fetch the list and the open thread again.
   */
  followCounts(counts: string): void {
    const seen = this.countsSeen;
    this.countsSeen = counts;
    if (seen === null || seen === counts) return;
    void this.reload();
    void this.refreshThread();
  }

  /** Fetch the shown list again, as far as it has been paged. A failure lands in `loadError`. */
  reload(): Promise<void> {
    const list = this.list;
    if (list === null) return Promise.resolve();
    const request = ++this.listRequest;
    const limit = Math.min(MAX_PAGE_SIZE, Math.max(PAGE_SIZE, this.threads.length));
    this.loadingMore = false;
    return (async () => {
      try {
        const page = await fetchMailThreads(query(list, limit));
        if (request !== this.listRequest) return;
        this.threads = page.threads;
        this.next = page.next;
        this.loaded = true;
        this.loadError = null;
      } catch (err) {
        if (request !== this.listRequest) return;
        this.loadError = userErrorMessage(err, { action: LOAD_FAILED[list.folder] });
      }
    })();
  }

  /** Fetch the next older page ("Show older"). A failure lands in `moreError`. */
  async loadMore(): Promise<void> {
    const { list, next: before } = this;
    if (list === null || before === null || this.loadingMore) return;
    const request = this.listRequest;
    this.loadingMore = true;
    try {
      const page = await fetchMailThreads(query(list, PAGE_SIZE, before));
      if (request !== this.listRequest) return;
      const held = this.threads.map((summary) => summary.thread_id);
      this.threads = [...this.threads, ...page.threads.filter((s) => !held.includes(s.thread_id))];
      this.next = page.next;
      this.moreError = null;
    } catch (err) {
      if (request !== this.listRequest) return;
      this.moreError = userErrorMessage(err, { action: "Couldn't load older conversations." });
    } finally {
      if (request === this.listRequest) this.loadingMore = false;
    }
  }

  /** The list's summary of a thread, when the loaded pages hold it. */
  summaryOf(threadId: string): MailThreadSummary | undefined {
    return this.threads.find((summary) => summary.thread_id === threadId);
  }

  // ── The open thread ────────────────────────────────────────────────

  /**
   * Open the thread `ref` names (the thread's id or any of its messages'
   * refs) and mark it read. The outcome carries the thread's own id, for the
   * address to name it by.
   */
  async open(ref: string): Promise<OpenThreadOutcome> {
    const held = this.thread;
    if (this.threadStatus === "ready" && held !== null && holdsRef(held, ref)) {
      this.openRef = ref;
      return { status: "shown", threadId: held.thread_id };
    }
    const request = ++this.openRequest;
    this.openRef = ref;
    this.thread = null;
    this.threadStatus = "loading";
    this.threadError = null;
    try {
      const thread = await fetchMailThread(ref);
      if (request !== this.openRequest) return { status: "superseded" };
      this.shown(thread);
      return { status: "shown", threadId: thread.thread_id };
    } catch (err) {
      if (request !== this.openRequest) return { status: "superseded" };
      if (err instanceof ApiError && err.status === 404) {
        this.threadStatus = "missing";
        return { status: "missing" };
      }
      this.threadStatus = "error";
      this.threadError = userErrorMessage(err, { action: "Couldn't open that conversation." });
      return { status: "failed" };
    }
  }

  /** Fetch the open thread again after a failure. */
  async retryOpen(): Promise<OpenThreadOutcome> {
    const ref = this.openRef;
    if (ref === null) return { status: "superseded" };
    this.thread = null;
    return this.open(ref);
  }

  /** No thread is open any more. Its reply draft stays for when it opens again. */
  close(): void {
    this.openRequest++;
    this.openRef = null;
    this.thread = null;
    this.threadStatus = "idle";
    this.threadError = null;
  }

  /** The reply draft of the open thread, or null while none is shown. */
  get openReply(): ReplyDraft | null {
    const thread = this.thread;
    return thread === null ? null : (this.replies[thread.thread_id] ?? null);
  }

  /** Fetch the open thread again in place, keeping it on screen; a failure keeps what is shown. */
  private async refreshThread(): Promise<void> {
    const held = this.thread;
    if (held === null || this.threadStatus !== "ready") return;
    const request = this.openRequest;
    try {
      const thread = await fetchMailThread(held.thread_id);
      if (request !== this.openRequest) return;
      this.shown(thread);
    } catch (err) {
      // What is shown is still the thread as it last stood; the next change tries again.
      userErrorMessage(err, { action: "Couldn't refresh the open conversation." });
    }
  }

  private shown(thread: MailThread): void {
    this.thread = thread;
    this.threadStatus = "ready";
    this.threadError = null;
    if (this.replies[thread.thread_id] === undefined) {
      this.replies = { ...this.replies, [thread.thread_id]: new ReplyDraft(thread.thread_id) };
    }
    const unread =
      thread.messages.some((message) => fromAgent(message) && !message.read) ||
      this.summaryOf(thread.thread_id)?.unread === true;
    if (unread) void this.markRead(thread.thread_id);
  }

  // ── Thread actions ─────────────────────────────────────────────────

  /**
   * Mark a thread read, at once in the list and the open thread; when the hub
   * refuses, both go back and `problems` says why.
   */
  async markRead(threadId: string): Promise<boolean> {
    if (this.reading.includes(threadId)) return false;
    this.reading = [...this.reading, threadId];
    this.problems = withoutKey(this.problems, threadId);
    const summary = this.summaryOf(threadId);
    const wasUnread = summary?.unread ?? false;
    const unreadRefs = (this.thread?.thread_id === threadId ? this.thread.messages : [])
      .filter((message) => fromAgent(message) && !message.read)
      .map((message) => message.ref);
    this.setRead(threadId, true, wasUnread, unreadRefs);
    try {
      await markMailThreadRead(threadId);
      return true;
    } catch (err) {
      this.setRead(threadId, false, wasUnread, unreadRefs);
      this.problems = {
        ...this.problems,
        [threadId]: { action: "read", message: actionFailed(err, "read") },
      };
      return false;
    } finally {
      this.reading = this.reading.filter((id) => id !== threadId);
    }
  }

  /** Move a thread's messages from agents to the archive. A failure lands in `problems`. */
  archive(threadId: string): Promise<boolean> {
    return this.move(threadId, "archive");
  }

  /** Move an archived thread back to the inbox: the inverse of `archive`, and its Undo. */
  restore(threadId: string): Promise<boolean> {
    return this.move(threadId, "restore");
  }

  /** Try a thread's failed action again. */
  async retry(threadId: string): Promise<boolean> {
    const problem = this.problems[threadId];
    if (problem === undefined) return false;
    switch (problem.action) {
      case "read":
        return this.markRead(threadId);
      case "archive":
        return this.archive(threadId);
      case "restore":
        return this.restore(threadId);
    }
  }

  private async move(threadId: string, action: "archive" | "restore"): Promise<boolean> {
    if (this.pending[threadId] !== undefined) return false;
    this.pending = { ...this.pending, [threadId]: action };
    this.problems = withoutKey(this.problems, threadId);
    try {
      await (action === "archive" ? archiveMailThread(threadId) : restoreMailThread(threadId));
      this.moved(threadId, action === "archive");
      return true;
    } catch (err) {
      this.problems = {
        ...this.problems,
        [threadId]: { action, message: actionFailed(err, action) },
      };
      return false;
    } finally {
      this.pending = withoutKey(this.pending, threadId);
    }
  }

  /**
   * A thread was archived or restored: its flags change in place, it leaves a
   * list it no longer belongs in, and a list it now belongs in but doesn't
   * hold (an Undo) is fetched again so it lands in its place.
   */
  private moved(threadId: string, archived: boolean): void {
    const thread = this.thread;
    if (thread?.thread_id === threadId) {
      this.thread = {
        ...thread,
        messages: thread.messages.map((message) =>
          fromAgent(message) ? { ...message, archived } : message,
        ),
      };
    }
    const list = this.list;
    const summary = this.summaryOf(threadId);
    if (list === null) return;
    if (summary === undefined) {
      if ((archived && list.folder === "archived") || (!archived && list.folder === "inbox")) {
        void this.reload();
      }
      return;
    }
    const updated = { ...summary, archived, in_inbox: !archived };
    this.threads = inFolder(updated, list.folder)
      ? this.threads.map((held) => (held.thread_id === threadId ? updated : held))
      : this.threads.filter((held) => held.thread_id !== threadId);
  }

  private setRead(
    threadId: string,
    read: boolean,
    wasUnread: boolean,
    refs: readonly string[],
  ): void {
    if (wasUnread) {
      this.threads = this.threads.map((summary) =>
        summary.thread_id === threadId ? { ...summary, unread: !read } : summary,
      );
    }
    const thread = this.thread;
    if (thread?.thread_id === threadId && refs.length > 0) {
      this.thread = {
        ...thread,
        messages: thread.messages.map((message) =>
          refs.includes(message.ref) ? { ...message, read } : message,
        ),
      };
    }
  }

  // ── Drafts ─────────────────────────────────────────────────────────

  /**
   * Open the New message, addressed to `to`. One already being written is
   * kept as it is, so a reload of the address doesn't lose it.
   */
  openNewMessage(to: readonly string[] = []): NewMessageDraft {
    const held = this.newMessage;
    if (held !== null) return held;
    const draft = new NewMessageDraft(to);
    this.newMessage = draft;
    return draft;
  }

  /** Throw the New message away. */
  discardNewMessage(): void {
    this.newMessage?.clear();
    this.newMessage = null;
  }

  /**
   * Send the New message once its uploads finish. Resolves to the thread it
   * started, or null with `sendError` set (or the draft invalid). The list is
   * fetched again so the thread shows in Sent.
   */
  async sendNewMessage(): Promise<string | null> {
    const draft = this.newMessage;
    if (draft === null || draft.sending) return null;
    const invalid = draft.invalid;
    if (invalid !== null) {
      draft.sendError = invalid;
      return null;
    }
    draft.sending = true;
    draft.sendError = null;
    try {
      const attachments = await draft.uploadIds();
      if (attachments === null) return null;
      const { thread_id } = await sendMail({
        to: [...draft.to],
        subject: draft.subject.trim(),
        body: draft.body,
        attachments,
      });
      if (this.newMessage === draft) this.newMessage = null;
      draft.clear();
      void this.reload();
      return thread_id;
    } catch (err) {
      draft.sendError = userErrorMessage(err, { action: "Couldn't send your message." });
      return null;
    } finally {
      draft.sending = false;
    }
  }

  /** The reply draft of a thread, when it has been opened. */
  replyDraft(threadId: string): ReplyDraft | undefined {
    return this.replies[threadId];
  }

  /** Throw a thread's reply draft away. */
  discardReply(threadId: string): void {
    this.replies[threadId]?.clear();
  }

  /**
   * Send a thread's reply to everyone in it once its uploads finish. The new
   * message joins the open thread and the list is fetched again. Resolves
   * whether it went; when it didn't, the draft's `sendError` says why.
   */
  async sendReply(threadId: string): Promise<boolean> {
    const draft = this.replies[threadId];
    if (draft === undefined || draft.sending) return false;
    const invalid = draft.invalid;
    if (invalid !== null) {
      draft.sendError = invalid;
      return false;
    }
    draft.sending = true;
    draft.sendError = null;
    try {
      const attachments = await draft.uploadIds();
      if (attachments === null) return false;
      const { message } = await replyToMailThread(threadId, { body: draft.body, attachments });
      const thread = this.thread;
      if (thread?.thread_id === threadId) {
        this.thread = { ...thread, messages: [...thread.messages, message] };
      }
      draft.clear();
      void this.reload();
      return true;
    } catch (err) {
      draft.sendError = userErrorMessage(err, {
        action: "Couldn't send your reply.",
        notFound: GONE,
      });
      return false;
    } finally {
      draft.sending = false;
    }
  }

  /** Some draft holds something the user wrote or attached. */
  get dirty(): boolean {
    return (
      this.newMessage?.dirty === true || Object.values(this.replies).some((draft) => draft.dirty)
    );
  }

  /**
   * What going to `target` would lose, for `router.guard` (null when the page
   * itself goes away): the New message when `target` doesn't keep it open,
   * and every reply when the page goes away. Replies otherwise stay with their
   * threads in memory.
   */
  draftLoss(target: AppLocation | null): string | null {
    const losses: string[] = [];
    const keepsNewMessage = target?.place.kind === "inbox" && target.place.compose !== null;
    if (this.newMessage?.dirty === true && !keepsNewMessage) losses.push("an unsent new message");
    if (target === null) {
      const replies = Object.values(this.replies).filter((draft) => draft.dirty).length;
      if (replies === 1) losses.push("an unsent reply");
      if (replies > 1) losses.push(`${String(replies)} unsent replies`);
    }
    if (losses.length === 0) return null;
    const line = losses.join(" and ");
    return line.charAt(0).toUpperCase() + line.slice(1);
  }
}

const LOAD_FAILED: Readonly<Record<MailFolder, string>> = {
  inbox: "Couldn't load your inbox.",
  sent: "Couldn't load your sent messages.",
  archived: "Couldn't load the archive.",
};

function actionFailed(err: unknown, action: ThreadAction): string {
  return userErrorMessage(err, { action: FAILED[action], notFound: GONE });
}

function query(
  list: MailList,
  limit: number,
  before?: string,
): Parameters<typeof fetchMailThreads>[0] {
  return { folder: list.folder, agent: list.agent ?? undefined, before, limit };
}

export const mail = new MailStore();
