import { describe, expect, it } from "vitest";
import { placeSubject, tabTitle, type TabTitleInput } from "./tab-title";
import type { Place } from "./routes";

const QUIET: TabTitleInput = {
  subject: "atlas",
  hidden: false,
  unread: 0,
  working: false,
  finished: false,
};

describe("tabTitle", () => {
  it("names the subject and the app", () => {
    expect(tabTitle(QUIET)).toBe("atlas · Residuum");
    expect(tabTitle({ ...QUIET, subject: null })).toBe("Residuum");
  });

  it("adds no markers while the tab is visible, whatever is going on", () => {
    expect(tabTitle({ ...QUIET, unread: 4, working: true, finished: true })).toBe(
      "atlas · Residuum",
    );
  });

  it("leads with the unread count while the tab is hidden", () => {
    expect(tabTitle({ ...QUIET, hidden: true, unread: 2 })).toBe("(2) atlas · Residuum");
    expect(tabTitle({ ...QUIET, hidden: true, unread: 2, subject: null })).toBe("(2) Residuum");
    expect(tabTitle({ ...QUIET, hidden: true, unread: 0 })).toBe("atlas · Residuum");
  });

  it("says the agent is working, or has finished, while the tab is hidden", () => {
    expect(tabTitle({ ...QUIET, hidden: true, working: true })).toBe("atlas is working · Residuum");
    expect(tabTitle({ ...QUIET, hidden: true, finished: true })).toBe("atlas finished · Residuum");
    expect(tabTitle({ ...QUIET, hidden: true, working: true, finished: true, unread: 1 })).toBe(
      "(1) atlas is working · Residuum",
    );
  });
});

describe("placeSubject", () => {
  const name = (agent: string): string => agent.toUpperCase();

  it.each<[Place, string | null]>([
    [{ kind: "home" }, null],
    [{ kind: "inbox", agent: "scout", tab: "active", item: null }, "Inbox"],
    [{ kind: "workbench", artifact: null }, "Workbench"],
    [{ kind: "shared-files" }, "Shared files"],
    [{ kind: "chat", agent: "atlas" }, "ATLAS"],
    [{ kind: "activity", agent: "atlas" }, "ATLAS"],
    [{ kind: "schedule", agent: "atlas" }, "ATLAS"],
    [{ kind: "files", agent: "atlas" }, "ATLAS"],
  ])("titles %j as %s", (place, subject) => {
    expect(placeSubject(place, name)).toBe(subject);
  });
});
