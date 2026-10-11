import { describe, expect, it } from "vitest";
import { isWorkspacePath } from "./markdown";

describe("isWorkspacePath", () => {
  it.each([
    "team/wiki/index.md",
    "notes/plan.md",
    "memory/2026-03-14_notes.v2.txt",
    "inbox/agent/réponse.md",
    "a/b.c",
  ])("takes %s as a path", (text) => {
    expect(isWorkspacePath(text)).toBe(true);
  });

  it.each([
    ["one segment", "config.toml"],
    ["no dot in the last segment", "team/wiki"],
    ["a leading slash", "/etc/hosts.conf"],
    ["a trailing slash", "team/wiki/"],
    ["a parent segment", "../secrets.md"],
    ["a current-directory segment", "./notes/plan.md"],
    ["a space", "team/my notes.md"],
    ["other punctuation", "team/wiki/a+b.md"],
    ["a doubled slash", "team//index.md"],
  ])("refuses %s", (_reason, text) => {
    expect(isWorkspacePath(text)).toBe(false);
  });
});
