import { describe, expect, it } from "vitest";
import { changeMatchesPrefix, changesUnder, normalizeWatchPrefix } from "./workspace-watch";

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
