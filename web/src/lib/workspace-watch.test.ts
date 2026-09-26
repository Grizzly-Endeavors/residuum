import { describe, expect, it } from "vitest";
import {
  changeMatchesPrefix,
  changesUnder,
  normalizeWatchPrefix,
  WorkspaceWatchSync,
} from "./workspace-watch";
import type { ClientMessage } from "./types";

describe("normalizeWatchPrefix", () => {
  it.each([
    ["wiki", "wiki"],
    ["wiki/", "wiki"],
    ["./wiki//sub/.", "wiki/sub"],
    ["", ""],
    [".", ""],
  ])("normalizes %j to %j", (prefix, expected) => {
    expect(normalizeWatchPrefix(prefix)).toBe(expected);
  });

  it.each(["../secrets", "wiki/../../etc", "/etc", "C:/Windows", "wiki\\a"])(
    "refuses %j",
    (prefix) => {
      expect(normalizeWatchPrefix(prefix)).toBeNull();
    },
  );
});

describe("changeMatchesPrefix", () => {
  it("matches by whole path segments", () => {
    expect(changeMatchesPrefix("wiki", "wiki")).toBe(true);
    expect(changeMatchesPrefix("wiki/a.md", "wiki")).toBe(true);
    expect(changeMatchesPrefix("wikipedia/a.md", "wiki")).toBe(false);
    expect(changeMatchesPrefix("wiki.md", "wiki")).toBe(false);
  });

  it("matches a folder containing the prefix, and everything for the whole workspace", () => {
    expect(changeMatchesPrefix("projects", "projects/alpha/notes")).toBe(true);
    expect(changeMatchesPrefix("projects/beta", "projects/alpha/notes")).toBe(false);
    expect(changeMatchesPrefix("any/where.md", "")).toBe(true);
  });

  it("filters a batch to the changes under any prefix", () => {
    const changes = [
      { path: "wiki/a.md", kind: "created" as const },
      { path: "inbox/user/x.md", kind: "modified" as const },
      { path: "notes/y.md", kind: "removed" as const },
    ];
    expect(changesUnder(changes, ["wiki", "inbox/user"]).map((c) => c.path)).toEqual([
      "wiki/a.md",
      "inbox/user/x.md",
    ]);
  });
});

describe("WorkspaceWatchSync", () => {
  function sync(): { watch: WorkspaceWatchSync; sent: ClientMessage[] } {
    const sent: ClientMessage[] = [];
    return { watch: new WorkspaceWatchSync((msg) => sent.push(msg)), sent };
  }

  it("sends the open artifact's watch set when it changes", () => {
    const { watch, sent } = sync();
    watch.set(["wiki", "inbox/user"]);
    watch.set(["inbox/user", "wiki"]);
    watch.set([]);
    expect(sent).toEqual([
      { type: "watch_workspace", prefixes: ["inbox/user", "wiki"] },
      { type: "watch_workspace", prefixes: [] },
    ]);
  });

  it("re-sends the watch set after every reconnect", () => {
    const { watch, sent } = sync();
    watch.connected();
    expect(sent).toEqual([]);

    watch.set(["wiki"]);
    watch.connected();
    watch.connected();
    expect(sent).toEqual([
      { type: "watch_workspace", prefixes: ["wiki"] },
      { type: "watch_workspace", prefixes: ["wiki"] },
      { type: "watch_workspace", prefixes: ["wiki"] },
    ]);
    expect(watch.current).toEqual(["wiki"]);
  });
});
