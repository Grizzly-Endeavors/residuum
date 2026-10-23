import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { hub } from "../../lib/hub.svelte";
import type { AgentOverview, AgentSummary, MailThreadSummary } from "../../lib/hub-types";
import { overview } from "../../lib/overview.svelte";
import type { ShellActions } from "../../shell/shell-actions";
import { render, screen } from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import NeedsYou from "./NeedsYou.svelte";

const NOW = Date.parse("2026-03-14T12:00:00Z");

const ACTIONS: ShellActions = {
  openSearch: () => undefined,
  openSettings: () => undefined,
  openShortcuts: () => undefined,
  openNotifications: () => undefined,
  openFeedback: () => undefined,
  createAgent: () => undefined,
};

const ATLAS: AgentSummary = {
  name: "atlas",
  display_name: "atlas",
  state: "running",
  last_error: null,
  autostart: true,
  role: null,
  a2a_visibility: "private",
  teams_configured: false,
};

function atlasWith(inboxUnread: number): AgentOverview {
  return {
    name: "atlas",
    last_message: null,
    live_sessions: [],
    upcoming: [],
    inbox_unread: inboxUnread,
    outbound_problems: [],
  };
}

function unreadThread(id: string): MailThreadSummary {
  return {
    thread_id: `atlas/user/${id}`,
    subject: id,
    participants: ["atlas"],
    message_count: 1,
    latest_at: "2026-03-14T11:00:00Z",
    snippet: "",
    unread: true,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
  };
}

/** The Inbox state the overview store holds: what its newest-page search found. */
function inboxHolds(threads: number, options: { inboxUnread: number; complete: boolean }): void {
  overview.overviews = { atlas: atlasWith(options.inboxUnread) };
  overview.unreadThreads = Array.from({ length: threads }, (_, n) => unreadThread(`t${String(n)}`));
  overview.unreadThreadsComplete = options.complete;
}

beforeEach(() => {
  hub.handleFrame(snapshot([ATLAS]));
});

afterEach(() => {
  overview.overviews = {};
  overview.unreadThreads = [];
  overview.unreadThreadsComplete = false;
});

describe("NeedsYou, under the unread threads", () => {
  it("counts the threads beyond the five listed when the whole inbox was searched", () => {
    inboxHolds(7, { inboxUnread: 12, complete: true });

    render(NeedsYou, { actions: ACTIONS, now: NOW });

    expect(screen.getByRole("link", { name: "2 more in Inbox" }).getAttribute("href")).toBe(
      "/inbox",
    );
  });

  it("says there is more without a number when the newest page wasn't the last", () => {
    inboxHolds(5, { inboxUnread: 9, complete: false });

    render(NeedsYou, { actions: ACTIONS, now: NOW });

    expect(screen.getByRole("link", { name: "More in Inbox" })).toBeTruthy();
  });

  it("says unread mail is waiting when none of it was on the newest page", () => {
    inboxHolds(0, { inboxUnread: 2, complete: false });

    render(NeedsYou, { actions: ACTIONS, now: NOW });

    expect(screen.getByRole("link", { name: "Unread in Inbox" })).toBeTruthy();
  });

  it("shows no link when every unread thread is listed", () => {
    inboxHolds(3, { inboxUnread: 4, complete: true });

    render(NeedsYou, { actions: ACTIONS, now: NOW });

    expect(screen.queryByRole("link", { name: /in Inbox/ })).toBeNull();
  });
});
