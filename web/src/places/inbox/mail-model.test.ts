import { describe, expect, it } from "vitest";
import type { MailThreadSummary } from "../../lib/hub-types";
import { agentMessage, mailSummary, mailThread } from "../../test/mail";
import { emptyListText, panelOfThread, participantsLabel } from "./mail-model";

const thread = mailThread(
  "atlas/user/a",
  [
    agentMessage("atlas/user/a", {
      attachments: [
        {
          filename: "notes.md",
          size: 10,
          content_type: "text/markdown",
          url: "/api/agents/atlas/inbox/a/attachments/0",
          agent: "atlas",
          path: "inbox/user/attachments/a/notes.md",
        },
      ],
    }),
  ],
  { participants: ["atlas"], recipients_for_reply: ["atlas"] },
);

describe("participantsLabel", () => {
  const withAgents = (participants: string[]): MailThreadSummary =>
    mailSummary("atlas/user/a", { participants });

  it("names the agents in order", () => {
    expect(participantsLabel(withAgents(["atlas", "scout"]), "inbox")).toBe("atlas, scout");
    expect(participantsLabel(withAgents(["atlas", "scout"]), "archived")).toBe("atlas, scout");
  });

  it("says Sent rows went to them", () => {
    expect(participantsLabel(withAgents(["atlas", "scout"]), "sent")).toBe("To atlas, scout");
  });

  it("reads You when no agent took part, in any folder", () => {
    expect(participantsLabel(withAgents([]), "inbox")).toBe("You");
    expect(participantsLabel(withAgents([]), "sent")).toBe("You");
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
