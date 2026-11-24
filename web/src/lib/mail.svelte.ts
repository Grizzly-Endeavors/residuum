import {
  ApiError,
  archiveMailThread,
  fetchMailThread,
  fetchMailThreads,
  markMailThreadRead,
  replyToMailThread,
  restoreMailThread,
  sendMail,
} from "./api";
import { userErrorMessage } from "./errors";
import type { MailFolder, MailMessage, MailThread, MailThreadSummary } from "./hub-types";
import type { AppLocation } from "./routes";
import { StagedAttachments } from "./staged-attachments.svelte";
import { toast } from "./toast.svelte";
import { joinNames } from "./words";

export interface MailList {
  folder: MailFolder;
  agent: string | null;
}

export type ThreadAction = "read" | "archive" | "restore";

export interface ThreadProblem {
  action: ThreadAction;
  message: string;
}

export type OpenThreadStatus = "idle" | "loading" | "ready" | "missing" | "error";

export type OpenThreadOutcome =
  | { status: "shown"; threadId: string }
  | { status: "missing" }
  | { status: "failed" }
  | { status: "superseded" };

export type ThreadMailbox = "inbox" | "archived" | null;

const PAGE_SIZE = 50;
const MAX_PAGE_SIZE = 200;

const FAILED: Readonly<Record<ThreadAction, string>> = {
  read: "Couldn't mark it read.",
  archive: "Couldn't archive it.",
  restore: "Couldn't move it to the inbox.",
};

const GONE = "That conversation isn't there any more.";

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

export function mailboxOf(thread: MailThread): ThreadMailbox {
  const agents = thread.messages.filter(fromAgent);
  if (agents.length === 0) return null;
  return agents.some((message) => !message.archived) ? "inbox" : "archived";
}

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

export class MailDraft {
  body = $state("");
  readonly files = new StagedAttachments();
  sending = $state(false);
  sendError = $state<string | null>(null);

  get dirty(): boolean {
    return this.body.trim() !== "" || this.files.attachments.length > 0;
  }

  get invalid(): string | null {
    return this.body.trim() === "" ? "Write a message before sending it." : null;
  }

  async uploadIds(): Promise<string[] | null> {
    await this.files.settled();
    const problem = this.files.problem;
    if (problem !== null) {
      this.sendError = problem;
      return null;
    }
    return this.files.uploadIds;
  }

  clear(): void {
    this.files.clear();
    this.body = "";
    this.sendError = null;
  }
}

export class ReplyDraft extends MailDraft {
  constructor(readonly threadId: string) {
    super();
  }

  override get invalid(): string | null {
    return this.body.trim() === "" ? "Write a reply before sending it." : null;
  }
}

export class NewMessageDraft extends MailDraft {
  to = $state<string[]>([]);
  subject = $state("");

  constructor(to: readonly string[] = []) {
    super();
    this.to = [...to];
  }

  override get dirty(): boolean {
    return super.dirty || this.subject.trim() !== "";
  }

  override get invalid(): string | null {
    if (this.to.length === 0) return "Add at least one agent to send it to.";
    return super.invalid;
  }
}

export class MailStore {
  list = $state.raw<MailList | null>(null);
  threads = $state<MailThreadSummary[]>([]);
  next = $state<string | null>(null);
  loaded = $state(false);
  loadError = $state<string | null>(null);
  loadingMore = $state(false);
  moreError = $state<string | null>(null);

  openRef = $state<string | null>(null);
  thread = $state<MailThread | null>(null);
  threadStatus = $state<OpenThreadStatus>("idle");
  threadError = $state<string | null>(null);

  pending = $state<Record<string, "archive" | "restore">>({});
  problems = $state<Record<string, ThreadProblem>>({});

  newMessage = $state<NewMessageDraft | null>(null);
  replies = $state<Record<string, ReplyDraft>>({});

  private listRequest = 0;
  private openRequest = 0;
  private listVersion = 0;
  private threadVersion = 0;
  private reloading: Promise<void> | null = null;
  private reading: string[] = [];
  private countsSeen: string | null = null;

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

  leave(): void {
    this.countsSeen = null;
  }

  followCounts(counts: string): void {
    const seen = this.countsSeen;
    this.countsSeen = counts;
    if (seen === null || seen === counts) return;
    void this.reload();
    void this.refreshThread();
  }

  reload(): Promise<void> {
    const list = this.list;
    if (list === null) return Promise.resolve();
    const request = ++this.listRequest;
    this.loadingMore = false;
    const fetching = this.fetchList(list, request);
    this.reloading = fetching;
    void fetching.then(() => {
      if (this.reloading === fetching) this.reloading = null;
    });
    return fetching;
  }

  private async fetchList(list: MailList, request: number): Promise<void> {
    try {
      for (;;) {
        const version = this.listVersion;
        const limit = Math.min(MAX_PAGE_SIZE, Math.max(PAGE_SIZE, this.threads.length));
        const page = await fetchMailThreads(query(list, limit));
        if (request !== this.listRequest) return;
        if (version !== this.listVersion) continue;
        this.threads = page.threads;
        this.next = page.next;
        this.loaded = true;
        this.loadError = null;
        return;
      }
    } catch (err) {
      if (request !== this.listRequest) return;
      this.loadError = userErrorMessage(err, { action: LOAD_FAILED[list.folder] });
    }
  }

  async loadMore(): Promise<void> {
    if (this.loadingMore || this.olderPage() === null) return;
    const request = this.listRequest;
    this.loadingMore = true;
    try {
      if (this.reloading !== null) await this.reloading;
      const older = this.olderPage();
      if (request !== this.listRequest || older === null) return;
      const page = await fetchMailThreads(query(older.list, PAGE_SIZE, older.before));
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

  private olderPage(): { list: MailList; before: string } | null {
    const { list, next } = this;
    return list === null || next === null ? null : { list, before: next };
  }

  summaryOf(threadId: string): MailThreadSummary | undefined {
    return this.threads.find((summary) => summary.thread_id === threadId);
  }

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

  async retryOpen(): Promise<OpenThreadOutcome> {
    const ref = this.openRef;
    if (ref === null) return { status: "superseded" };
    this.thread = null;
    return this.open(ref);
  }

  close(): void {
    this.openRequest++;
    this.openRef = null;
    this.thread = null;
    this.threadStatus = "idle";
    this.threadError = null;
  }

  get openReply(): ReplyDraft | null {
    const thread = this.thread;
    return thread === null ? null : (this.replies[thread.thread_id] ?? null);
  }

  private async refreshThread(): Promise<void> {
    const held = this.thread;
    if (held === null || this.threadStatus !== "ready") return;
    const request = this.openRequest;
    try {
      for (;;) {
        const version = this.threadVersion;
        const thread = await fetchMailThread(held.thread_id);
        if (request !== this.openRequest) return;
        if (version !== this.threadVersion) continue;
        this.shown(thread);
        return;
      }
    } catch (err) {
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

  archive(threadId: string): Promise<boolean> {
    return this.move(threadId, "archive");
  }

  restore(threadId: string): Promise<boolean> {
    return this.move(threadId, "restore");
  }

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

  private moved(threadId: string, archived: boolean): void {
    this.listVersion++;
    this.threadVersion++;
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
    const updated = {
      ...summary,
      archived,
      in_inbox: !archived,
      unread: !archived && summary.unread,
    };
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
    this.listVersion++;
    this.threadVersion++;
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

  openNewMessage(to: readonly string[] = []): NewMessageDraft {
    const held = this.newMessage;
    if (held === null) {
      const draft = new NewMessageDraft(to);
      this.newMessage = draft;
      return draft;
    }
    if (!held.dirty) held.to = [...to];
    else held.to = [...held.to, ...to.filter((name) => !held.to.includes(name))];
    return held;
  }

  discardNewMessage(): void {
    const draft = this.newMessage;
    this.newMessage = null;
    if (draft !== null && !draft.sending) draft.clear();
  }

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
      if (attachments === null) {
        if (this.newMessage !== draft) {
          toast.error(
            `Your message to ${joinNames(draft.to)} wasn't sent because a file didn't upload.`,
          );
        }
        return null;
      }
      const recipients = [...draft.to];
      const { thread_id } = await sendMail({
        to: recipients,
        subject: draft.subject.trim(),
        body: draft.body,
        attachments,
      });
      const stillOpen = this.newMessage === draft;
      if (stillOpen) this.newMessage = null;
      draft.clear();
      void this.reload();
      if (stillOpen) return thread_id;
      toast.success(`Sent to ${joinNames(recipients)}.`);
      return null;
    } catch (err) {
      if (this.newMessage === draft) {
        draft.sendError = userErrorMessage(err, { action: "Couldn't send your message." });
      } else {
        toast.error(
          userErrorMessage(err, { action: `Your message to ${joinNames(draft.to)} wasn't sent.` }),
        );
      }
      return null;
    } finally {
      draft.sending = false;
    }
  }

  replyDraft(threadId: string): ReplyDraft | undefined {
    return this.replies[threadId];
  }

  discardReply(threadId: string): void {
    this.replies[threadId]?.clear();
  }

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
      this.threadVersion++;
      const thread = this.thread;
      if (thread?.thread_id === threadId && !thread.messages.some((m) => m.ref === message.ref)) {
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

  get dirty(): boolean {
    return (
      this.newMessage?.dirty === true || Object.values(this.replies).some((draft) => draft.dirty)
    );
  }

  draftLoss(target: AppLocation | null): string | null {
    const losses: string[] = [];
    const keepsNewMessage = target?.place.kind === "inbox" && target.place.compose !== null;
    const writing = this.newMessage;
    const goingAnyway = target !== null && writing?.sending === true;
    if (writing?.dirty === true && !goingAnyway && !keepsNewMessage) {
      losses.push("an unsent new message");
    }
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
