// ── The Inbox (Svelte 5 runes) ───────────────────────────────────────
//
// One list across every agent's user inbox, through the hub's inbox
// endpoints: the list the Inbox place shows (one filter and tab at a time),
// its older pages, and reading, archiving and restoring items. Counts aren't
// kept here: the overview store has them, and the list is fetched again when
// they change. Every failure lands in state the place shows with Try again.

import {
  ApiError,
  archiveHubInboxItem,
  fetchHubInbox,
  markHubInboxItemRead,
  restoreHubInboxItem,
} from "./api";
import { userErrorMessage } from "./errors";
import type { HubInboxItem } from "./hub-types";
import type { InboxItemRef, InboxTab } from "./routes";

/** Which list the Inbox shows: one agent's items or everyone's, in the inbox or the archive. */
export interface InboxList {
  agent: string | null;
  tab: InboxTab;
}

/** What can be done to one item. */
export type InboxItemAction = "read" | "archive" | "restore";

/** An action on an item that failed, and why, in plain words. */
export interface InboxItemProblem {
  action: InboxItemAction;
  message: string;
}

/**
 * How opening an item went: shown (or overtaken by opening another), gone
 * from the agent's inbox, or not reachable right now.
 */
export type InboxOpenOutcome = "shown" | "missing" | "failed";

/** How many items one page asks for. */
const PAGE_SIZE = 50;
/** The most one request may ask for, so a reload of a long-paged list stays one request. */
const MAX_PAGE_SIZE = 200;

const FAILED: Readonly<Record<InboxItemAction, string>> = {
  read: "Couldn't mark it read.",
  archive: "Couldn't archive it.",
  restore: "Couldn't move it back to the inbox.",
};

const GONE: Readonly<Record<InboxItemAction, string>> = {
  read: "It's no longer in the agent's inbox.",
  archive: "It's no longer in the inbox. It may have been archived already.",
  restore: "It's no longer in the archive. It may have been moved back already.",
};

/** An item's key: ids are unique within one agent's inbox, so the pair is unique. */
export function inboxItemKey(item: InboxItemRef): string {
  return `${item.agent}:${item.id}`;
}

function sameList(a: InboxList, b: InboxList): boolean {
  return a.agent === b.agent && a.tab === b.tab;
}

export class InboxStore {
  /** The list being shown, or null before the Inbox first opens. */
  list = $state.raw<InboxList | null>(null);
  /** The list's items, newest first, as far as it has been paged. */
  items = $state<HubInboxItem[]>([]);
  /** Where the next older page starts, or null when the list ends there. */
  nextCursor = $state<string | null>(null);
  /** The list's first page has arrived. */
  loaded = $state(false);
  /** Why the list couldn't be fetched, or null. */
  loadError = $state<string | null>(null);
  loadingMore = $state(false);
  /** Why an older page couldn't be fetched, or null. */
  moreError = $state<string | null>(null);

  /** The open item, when the loaded pages don't hold it: a link to an older or archived item. */
  openedApart = $state<HubInboxItem | null>(null);
  /** Why the open item couldn't be fetched, or null. */
  openError = $state<string | null>(null);

  /** The action in flight on each item, by key. */
  pending = $state<Record<string, InboxItemAction>>({});
  /** The last failed action on each item, by key, until it is tried again. */
  problems = $state<Record<string, InboxItemProblem>>({});

  private listRequest = 0;
  private openRequest = 0;
  private loading: Promise<void> = Promise.resolve();
  /** The unread counts the list was last fetched for; null while the Inbox isn't open. */
  private countsSeen: string | null = null;

  /**
   * Show `list` and fetch it. The items already held stay on screen while the
   * same list is fetched again; another list starts empty.
   */
  show(list: InboxList): Promise<void> {
    if (this.list === null || !sameList(this.list, list)) {
      this.list = list;
      this.items = [];
      this.nextCursor = null;
      this.loaded = false;
      this.loadError = null;
      this.problems = {};
    }
    this.moreError = null;
    return this.reload();
  }

  /** The Inbox closed: stop following the counts until it opens again. */
  leave(): void {
    this.countsSeen = null;
  }

  /** The unread counts as the overview has them: when they change, fetch the list again. */
  followCounts(counts: string): void {
    const seen = this.countsSeen;
    this.countsSeen = counts;
    if (seen !== null && seen !== counts) void this.reload();
  }

  /** Fetch the shown list again, as far as it has been paged. A failure lands in `loadError`. */
  reload(): Promise<void> {
    const list = this.list;
    if (list === null) return Promise.resolve();
    const request = ++this.listRequest;
    const limit = Math.min(MAX_PAGE_SIZE, Math.max(PAGE_SIZE, this.items.length));
    this.loadingMore = false;
    this.loading = (async () => {
      try {
        const page = await fetchHubInbox(query(list, limit));
        if (request !== this.listRequest) return;
        this.items = page.items;
        this.nextCursor = page.next_cursor;
        this.loaded = true;
        this.loadError = null;
      } catch (err) {
        if (request !== this.listRequest) return;
        this.loadError = userErrorMessage(err, {
          action:
            list.tab === "archived" ? "Couldn't load the archive." : "Couldn't load your inbox.",
        });
      }
    })();
    return this.loading;
  }

  /** Fetch the next older page. A failure lands in `moreError`. */
  async loadMore(): Promise<void> {
    const { list, nextCursor: before } = this;
    if (list === null || before === null || this.loadingMore) return;
    const request = this.listRequest;
    this.loadingMore = true;
    try {
      const page = await fetchHubInbox(query(list, PAGE_SIZE, before));
      if (request !== this.listRequest) return;
      const held = this.items.map(inboxItemKey);
      const older = page.items.filter((item) => !held.includes(inboxItemKey(item)));
      this.items = [...this.items, ...older];
      this.nextCursor = page.next_cursor;
      this.moreError = null;
    } catch (err) {
      if (request !== this.listRequest) return;
      this.moreError = userErrorMessage(err, { action: "Couldn't load older items." });
    } finally {
      if (request === this.listRequest) this.loadingMore = false;
    }
  }

  /** The item `ref` names, from the loaded pages or fetched apart from them. */
  find(ref: InboxItemRef): HubInboxItem | undefined {
    const key = inboxItemKey(ref);
    const held = this.items.find((item) => inboxItemKey(item) === key);
    if (held !== undefined) return held;
    return this.openedApart !== null && inboxItemKey(this.openedApart) === key
      ? this.openedApart
      : undefined;
  }

  /**
   * The item `ref` was opened: mark it read. When the list doesn't hold it, it
   * is fetched on its own, which marks it read too.
   */
  async open(ref: InboxItemRef): Promise<InboxOpenOutcome> {
    const request = ++this.openRequest;
    this.openError = null;
    await this.loading;
    if (request !== this.openRequest) return "shown";
    const held = this.items.find((item) => inboxItemKey(item) === inboxItemKey(ref));
    if (held !== undefined) {
      this.openedApart = null;
      if (!held.read) await this.markRead(held);
      return "shown";
    }
    try {
      const item = await markHubInboxItemRead(ref.agent, ref.id);
      if (request !== this.openRequest) return "shown";
      this.openedApart = item;
      return "shown";
    } catch (err) {
      if (request !== this.openRequest) return "shown";
      this.openedApart = null;
      if (err instanceof ApiError && err.status === 404) return "missing";
      this.openError = userErrorMessage(err, { action: "Couldn't open that item." });
      return "failed";
    }
  }

  /** Mark an item read. A failure lands in `problems`. */
  async markRead(item: InboxItemRef): Promise<boolean> {
    return this.act(item, "read", async () => {
      this.replace(await markHubInboxItemRead(item.agent, item.id));
    });
  }

  /** Move an item to the archive. A failure lands in `problems`. */
  async archive(item: InboxItemRef): Promise<boolean> {
    return this.act(item, "archive", async () => {
      this.moved(await archiveHubInboxItem(item.agent, item.id), "archived");
    });
  }

  /** Move an archived item back to the inbox. A failure lands in `problems`. */
  async restore(item: InboxItemRef): Promise<boolean> {
    return this.act(item, "restore", async () => {
      this.moved(await restoreHubInboxItem(item.agent, item.id), "active");
    });
  }

  /** Try an item's failed action again. */
  async retry(item: InboxItemRef): Promise<boolean> {
    const problem = this.problems[inboxItemKey(item)];
    if (problem === undefined) return false;
    switch (problem.action) {
      case "read":
        return this.markRead(item);
      case "archive":
        return this.archive(item);
      case "restore":
        return this.restore(item);
    }
  }

  private async act(
    item: InboxItemRef,
    action: InboxItemAction,
    call: () => Promise<void>,
  ): Promise<boolean> {
    const key = inboxItemKey(item);
    if (this.pending[key] !== undefined) return false;
    this.pending = { ...this.pending, [key]: action };
    this.problems = withoutKey(this.problems, key);
    try {
      await call();
      return true;
    } catch (err) {
      const message = userErrorMessage(err, { action: FAILED[action], notFound: GONE[action] });
      this.problems = { ...this.problems, [key]: { action, message } };
      return false;
    } finally {
      this.pending = withoutKey(this.pending, key);
    }
  }

  /** Put the hub's copy of an item in place of the one held. */
  private replace(item: HubInboxItem): void {
    const key = inboxItemKey(item);
    this.items = this.items.map((held) => (inboxItemKey(held) === key ? item : held));
    if (this.openedApart !== null && inboxItemKey(this.openedApart) === key) {
      this.openedApart = item;
    }
  }

  /**
   * An item moved to `tab`. Shown the list it left, drop it; shown the list it
   * joined (an Undo), fetch that list again so the item lands in its place.
   */
  private moved(item: HubInboxItem, tab: InboxTab): void {
    const key = inboxItemKey(item);
    if (this.list?.tab === tab) {
      void this.reload();
      return;
    }
    this.items = this.items.filter((held) => inboxItemKey(held) !== key);
    if (this.openedApart !== null && inboxItemKey(this.openedApart) === key) {
      this.openedApart = null;
    }
  }
}

function query(
  list: InboxList,
  limit: number,
  before?: string,
): Parameters<typeof fetchHubInbox>[0] {
  return {
    status: list.tab,
    agent: list.agent ?? undefined,
    before,
    limit,
  };
}

function withoutKey<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) return record;
  return Object.fromEntries(Object.entries(record).filter(([k]) => k !== key));
}

export const inbox = new InboxStore();
