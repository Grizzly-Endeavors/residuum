import { describe, expect, it } from "vitest";
import { isSocketPath, watchPrefixProblem } from "./sockets";

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
