import { describe, expect, it } from "vitest";
import { argumentLines } from "./step-args";

describe("a step's arguments", () => {
  it("summarize a known tool: what it acted on first, then what qualifies it", () => {
    expect(argumentLines("exec", { command: "git status", timeout_secs: 30 })).toEqual([
      { kind: "code", text: "$ git status" },
      { kind: "meta", text: "timeout: 30" },
    ]);
    expect(argumentLines("read_file", { path: "a/b.md", offset: 10, limit: 20 })).toEqual([
      { kind: "path", text: "a/b.md" },
      { kind: "meta", text: "lines 10–30" },
    ]);
    expect(
      argumentLines("memory_search", { query: "fallback", source: "episodes", limit: 5 }),
    ).toEqual([
      { kind: "query", text: "fallback" },
      { kind: "meta", text: "Source: episodes · Limit: 5" },
    ]);
  });

  it("keep long text a tool carries, to be clamped", () => {
    expect(argumentLines("subagent_spawn", { task: "Compare the strategies" })).toEqual([
      { kind: "quote", text: "Compare the strategies" },
    ]);
  });

  it("list every argument by name for a tool without a summary", () => {
    expect(argumentLines("lookup_weather", { city: "Oslo", days: 3, units: null })).toEqual([
      {
        kind: "pairs",
        pairs: [
          ["city", "Oslo"],
          ["days", "3"],
        ],
      },
    ]);
  });

  it("show nothing for a call that took no arguments", () => {
    expect(argumentLines("inbox_list", {})).toEqual([]);
  });
});
