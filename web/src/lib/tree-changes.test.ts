import { describe, expect, it } from "vitest";
import { treeUpdateFor, treeWatchPrefix } from "./tree-changes";
import type { WorkspaceChange } from "./types";

const created = (path: string): WorkspaceChange => ({ path, kind: "created" });
const removed = (path: string): WorkspaceChange => ({ path, kind: "removed" });
const modified = (path: string): WorkspaceChange => ({ path, kind: "modified" });

describe("treeWatchPrefix", () => {
  it("covers the whole tree: everything for an agent, the team folder for the team", () => {
    expect(treeWatchPrefix("agent")).toBe("");
    expect(treeWatchPrefix("team")).toBe("team");
  });
});

describe("treeUpdateFor", () => {
  it("lists again the listed folder that gained or lost an entry", () => {
    const update = treeUpdateFor([created("notes/new.md"), removed("old.md")], "agent", [
      "",
      "notes",
      "memory",
    ]);
    expect(update).toEqual({ reload: ["", "notes"], forget: [] });
  });

  it("ignores a change to a file's content, which the tree does not show", () => {
    expect(treeUpdateFor([modified("notes/a.md")], "agent", ["", "notes"])).toEqual({
      reload: [],
      forget: [],
    });
  });

  it("ignores a change in a folder the tree has not listed", () => {
    expect(treeUpdateFor([created("memory/2026/a.md")], "agent", ["", "notes"])).toEqual({
      reload: [],
      forget: [],
    });
  });

  it("forgets a removed folder and the listings under it, and does not list them again", () => {
    const update = treeUpdateFor([removed("projects"), removed("projects/alpha/a.md")], "agent", [
      "",
      "projects",
      "projects/alpha",
      "notes",
    ]);
    expect(update).toEqual({ reload: [""], forget: ["projects", "projects/alpha"] });
  });

  it("does not take a folder that shares a name prefix with a removed one", () => {
    const update = treeUpdateFor([removed("wiki")], "agent", ["", "wikipedia"]);
    expect(update).toEqual({ reload: [""], forget: [] });
  });

  it("reads the team tree's paths relative to team/", () => {
    const update = treeUpdateFor(
      [created("team/wiki/a.md"), created("team/b.md"), created("notes/c.md")],
      "team",
      ["", "wiki", "notes"],
    );
    expect(update).toEqual({ reload: ["", "wiki"], forget: [] });
  });

  it("ignores the team folder itself and anything outside it", () => {
    expect(treeUpdateFor([modified("team"), created("team")], "team", [""])).toEqual({
      reload: [],
      forget: [],
    });
    expect(treeUpdateFor([created("teams/a.md")], "team", [""])).toEqual({
      reload: [],
      forget: [],
    });
  });

  it("finds the team's folders in an agent's tree by their team/ paths", () => {
    const update = treeUpdateFor([created("team/wiki/a.md")], "agent", ["", "team", "team/wiki"]);
    expect(update).toEqual({ reload: ["team/wiki"], forget: [] });
  });
});
