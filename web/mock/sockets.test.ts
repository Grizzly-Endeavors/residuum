import { describe, expect, it } from "vitest";
import type { WorkspaceChange } from "../src/lib/generated/protocol";
import {
  isSocketPath,
  isWorkspaceFrame,
  normalizeWatchPrefix,
  watchedFrame,
  watchPrefixProblem,
} from "./sockets";

const change = (path: string): WorkspaceChange => ({ path, kind: "modified" });

describe("isSocketPath", () => {
  it("matches the path, with or without a query string", () => {
    expect(isSocketPath("/api/hub/ws", "/api/hub/ws")).toBe(true);
    expect(isSocketPath("/api/hub/ws?token=1", "/api/hub/ws")).toBe(true);
  });

  it("leaves other upgrades, like Vite's HMR socket, alone", () => {
    expect(isSocketPath("/", "/api/hub/ws")).toBe(false);
    expect(isSocketPath("/__vite_hmr", "/api/hub/ws")).toBe(false);
    expect(isSocketPath("/api/hub/ws/extra", "/api/hub/ws")).toBe(false);
    expect(isSocketPath("/api/hub/wsx", "/api/hub/ws")).toBe(false);
    expect(isSocketPath(undefined, "/api/hub/ws")).toBe(false);
  });
});

describe("watchPrefixProblem", () => {
  it.each(["", "team", "team/wiki", "wiki/notes", "a/./b", "a/b/"])("accepts %j", (prefix) => {
    expect(watchPrefixProblem(prefix)).toBeNull();
  });

  it.each([
    ["/etc", "watch paths are relative to the workspace"],
    ["C:/Users", "watch paths are relative to the workspace"],
    ["a/../b", 'watch paths must stay inside the workspace (no "..")'],
    ["..", 'watch paths must stay inside the workspace (no "..")'],
    ["a\\b", "it contains a character that can't appear in a workspace path"],
    ["a\0b", "it contains a character that can't appear in a workspace path"],
  ])("refuses %j", (prefix, reason) => {
    expect(watchPrefixProblem(prefix)).toContain(reason);
    expect(watchPrefixProblem(prefix)).toContain(`can't watch ${JSON.stringify(prefix)}`);
  });
});

describe("normalizeWatchPrefix", () => {
  it.each([
    ["team/wiki", "team/wiki"],
    ["team//wiki/", "team/wiki"],
    ["./team/./wiki", "team/wiki"],
    ["", ""],
    [".", ""],
  ])("turns %j into %j", (prefix, normalized) => {
    expect(normalizeWatchPrefix(prefix)).toBe(normalized);
  });
});

describe("watchedFrame", () => {
  const changed = (...paths: string[]): Parameters<typeof watchedFrame>[1] => ({
    type: "workspace_changed",
    changes: paths.map(change),
  });

  it("keeps the changes at, under or above a watched prefix, by whole segments, sorted by path", () => {
    const frame = changed(
      "team/wiki/b.md",
      "team/wikipedia/a.md",
      "team/wiki",
      "team/wiki.md",
      "team/wiki/a.md",
      "memory/a.md",
    );
    expect(watchedFrame(["team/wiki"], frame)).toEqual(
      changed("team/wiki", "team/wiki/a.md", "team/wiki/b.md"),
    );
    // Removing a folder carries the prefixes inside it along.
    expect(watchedFrame(["team/wiki/projects/index.md"], changed("team/wiki"))).toEqual(
      changed("team/wiki"),
    );
  });

  it("matches a file prefix only against that file", () => {
    expect(watchedFrame(["inbox/user/today.md"], changed("inbox/user/today.md.bak"))).toBeNull();
    expect(watchedFrame(["inbox/user/today.md"], changed("inbox/user/today.md"))).not.toBeNull();
  });

  it("gives the agent's own prefixes the agent's changes and the team's the team's", () => {
    expect(watchedFrame(["workbench"], changed("team/workbench/x.html"))).toBeNull();
    expect(watchedFrame(["team/workbench"], changed("workbench/x.html"))).toBeNull();
    expect(watchedFrame([""], changed("team/a.md", "a.md"))).toEqual(changed("a.md", "team/a.md"));
  });

  it("sends nothing to a connection that watches nothing, or no matching change", () => {
    expect(watchedFrame([], changed("team/a.md"))).toBeNull();
    expect(watchedFrame(["team/wiki"], changed("team/workbench/a.html"))).toBeNull();
  });

  it("sends the other change feed frames to a connection that watches anything", () => {
    const resync = { type: "workspace_resync", reason: "overflow" } as const;
    expect(watchedFrame(["team"], resync)).toEqual(resync);
    expect(watchedFrame([], resync)).toBeNull();
  });
});

describe("isWorkspaceFrame", () => {
  it("tells the change feed's frames from the rest", () => {
    expect(isWorkspaceFrame({ type: "workspace_changed", changes: [] })).toBe(true);
    expect(isWorkspaceFrame({ type: "workspace_resync", reason: "overflow" })).toBe(true);
    expect(isWorkspaceFrame({ type: "workspace_watch_unavailable", message: "off" })).toBe(true);
    expect(isWorkspaceFrame({ type: "artifact_updated", name: "x" })).toBe(false);
    expect(isWorkspaceFrame({ type: "hub_boot", boot_id: "b" })).toBe(false);
  });
});
