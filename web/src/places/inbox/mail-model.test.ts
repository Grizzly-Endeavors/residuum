import { describe, expect, it } from "vitest";
import type { MailMessage, MailThread, MailThreadSummary } from "../../lib/hub-types";
import { emptyListText, panelOfThread, participantsLabel } from "./mail-model";

function summary(participants: string[]): MailThreadSummary {
  return {
    thread_id: "atlas/user/a",
    subject: "s",
    participants,
    message_count: 1,
    latest_at: "2026-03-14T12:00:00Z",
    snippet: "",
    unread: false,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
  };
}

function message(attachments: MailMessage["attachments"]): MailMessage {
  return {
    ref: "atlas/user/a",
    from: { kind: "agent", agent: "atlas" },
    to: [],
    at: "2026-03-14T12:00:00Z",
    title: "s",
    body: "b",
    attachments,
    in_reply_to: null,
    read: true,
    archived: false,
    seen_by: [],
  };
}

const thread: MailThread = {
  thread_id: "atlas/user/a",
  subject: "s",
  participants: ["atlas"],
  messages: [
    message([
      {
        filename: "notes.md",
        size: 10,
        content_type: "text/markdown",
        url: "/api/agents/atlas/inbox/a/attachments/0",
        agent: "atlas",
        path: "inbox/user/attachments/a/notes.md",
      },
    ]),
  ],
  recipients_for_reply: ["atlas"],
};

describe("participantsLabel", () => {
  it("names the agents in order", () => {
    expect(participantsLabel(summary(["atlas", "scout"]))).toBe("atlas, scout");
  });

  it("reads You when no agent took part", () => {
    expect(participantsLabel(summary([]))).toBe("You");
  });
});

describe("emptyListText", () => {
  it("has its own words for each folder, with and without a filter", () => {
    const all = (["inbox", "sent", "archived"] as const).flatMap((folder) => [
      emptyListText(folder, null),
      emptyListText(folder, "atlas"),
    ]);
    expect(new Set(all).size).toBe(6);
    expect(emptyListText("inbox", "atlas")).toContain("atlas");
    expect(emptyListText("sent", "atlas")).toContain("atlas");
    expect(emptyListText("archived", "atlas")).toContain("atlas");
  });
});

describe("panelOfThread", () => {
  const path = "inbox/user/attachments/a/notes.md";

  it("owns an agent file panel that shows one of its attachments", () => {
    expect(panelOfThread(thread, { kind: "file", agent: "atlas", path })).toBe(true);
  });

  it("leaves another agent's file, another path, a place file panel and other panels alone", () => {
    expect(panelOfThread(thread, { kind: "file", agent: "scout", path })).toBe(false);
    expect(panelOfThread(thread, { kind: "file", agent: "atlas", path: "notes.md" })).toBe(false);
    expect(panelOfThread(thread, { kind: "file", agent: null, path })).toBe(false);
    expect(panelOfThread(thread, { kind: "size" })).toBe(false);
    expect(panelOfThread(thread, null)).toBe(false);
    expect(panelOfThread(null, { kind: "file", agent: "atlas", path })).toBe(false);
  });
});
