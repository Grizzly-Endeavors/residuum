import { describe, expect, it } from "vitest";
import { TOOL_RESULT_MARKER } from "./feed-items";
import {
  TOOL_RESULT_PREVIEW_CHARS,
  TOOL_RESULT_PREVIEW_LINES,
  formatToolResult,
  toolResultToggleLabel,
  type FormattedToolResult,
} from "./format-tool-result";

function expectShape<S extends FormattedToolResult["shape"]>(
  result: FormattedToolResult,
  shape: S,
): Extract<FormattedToolResult, { shape: S }> {
  if (result.shape !== shape) {
    throw new Error(`expected shape ${shape}, got ${result.shape}`);
  }
  // The shape check narrows at runtime; the generic comparison does not narrow the union.
  return result as Extract<FormattedToolResult, { shape: S }>;
}

describe("formatToolResult", () => {
  it("leaves ordinary text unchanged", () => {
    const result = expectShape(formatToolResult("wrote 12 bytes to notes.txt"), "text");
    expect(result.text).toBe("wrote 12 bytes to notes.txt");
    expect(result.preview).toBe(result.text);
    expect(result.error).toBe(false);
    expect(result.collapse.long).toBe(false);
  });

  it("strips the feed's result marker", () => {
    const result = expectShape(formatToolResult(`${TOOL_RESULT_MARKER}no results found`), "text");
    expect(result.text).toBe("no results found");
  });

  it("formats each marked chunk and joins them", () => {
    const raw = `${TOOL_RESULT_MARKER}{"ok":true}\n${TOOL_RESULT_MARKER}done`;
    const result = expectShape(formatToolResult(raw), "text");
    expect(result.text).toBe('{\n  "ok": true\n}\n\ndone');
  });

  it("returns empty text for a blank result", () => {
    const result = expectShape(formatToolResult("  \n"), "text");
    expect(result.text).toBe("");
    expect(result.collapse.long).toBe(false);
  });

  it("pretty-prints a JSON object and keeps key order", () => {
    const result = expectShape(formatToolResult('{"b":1,"a":[2,3]}'), "json");
    expect(result.text).toBe('{\n  "b": 1,\n  "a": [\n    2,\n    3\n  ]\n}');
    expect(result.label).toBe("");
    expect(result.error).toBe(false);
  });

  it("pretty-prints a JSON array", () => {
    const result = expectShape(formatToolResult("[1,2]"), "json");
    expect(result.text).toBe("[\n  1,\n  2\n]");
  });

  it("keeps a fetch preamble and pretty-prints the JSON that follows it", () => {
    const raw = 'total 20 bytes\ncontent-type: application/json\n\n{"ok":true}';
    const result = expectShape(formatToolResult(raw), "json");
    expect(result.label).toBe("total 20 bytes\ncontent-type: application/json");
    expect(result.text).toBe('{\n  "ok": true\n}');
  });

  it("leaves JSON primitives and invalid JSON as text", () => {
    expect(formatToolResult('"hello"').shape).toBe("text");
    expect(formatToolResult("42").shape).toBe("text");
    expect(formatToolResult("{not json").shape).toBe("text");
    expect(expectShape(formatToolResult("{not json"), "text").text).toBe("{not json");
  });

  it("normalizes CRLF before detecting JSON", () => {
    const result = expectShape(formatToolResult('{"ok":true}\r\n'), "json");
    expect(result.text).toBe('{\n  "ok": true\n}');
  });

  it("splits a read_file dump into a header and numbered rows", () => {
    const raw = ["file: 12 bytes, 2 line(s) total", "", "   1\thello", "   2\tworld"].join("\n");
    const result = expectShape(formatToolResult(raw), "file");
    expect(result.label).toBe("file: 12 bytes, 2 line(s) total");
    expect(result.rows).toEqual([
      { kind: "code", number: "1", text: "hello" },
      { kind: "code", number: "2", text: "world" },
    ]);
    expect(result.error).toBe(false);
  });

  it("keeps a read_file range note in the header", () => {
    const raw = [
      "file: 40 bytes, 4 line(s) total",
      "showing lines 2-3 of 4; use offset/limit to see more",
      "",
      "   2\tone",
      "   3\ttwo",
    ].join("\n");
    const result = expectShape(formatToolResult(raw), "file");
    expect(result.label).toContain("showing lines 2-3 of 4");
    expect(result.rows).toHaveLength(2);
  });

  it("reads an edit preview as a file, including the gap between regions", () => {
    const raw = ["edited src/main.rs (1 replacement)", "", "  10\told", "   …", "  20\tnew"].join(
      "\n",
    );
    const result = expectShape(formatToolResult(raw), "file");
    expect(result.label).toBe("edited src/main.rs (1 replacement)");
    expect(result.rows.map((row) => row.kind)).toEqual(["code", "gap", "code"]);
  });

  it("does not treat a single numbered line without a file header as a file", () => {
    const result = expectShape(formatToolResult("note\n   1\tonly"), "text");
    expect(result.text).toContain("only");
  });

  it("groups a bulleted list into items", () => {
    const result = expectShape(formatToolResult("- alpha\n- beta\n- gamma"), "list");
    expect(result.label).toBe("");
    expect(result.items).toEqual(["- alpha", "- beta", "- gamma"]);
  });

  it("groups numbered search results, keeping indented snippets on the item", () => {
    const raw = [
      "Found 2 result(s):",
      "",
      "1. [observations] ep-1 | 2026-01-01 (score: 0.87)",
      "   first snippet",
      "",
      "2. [wiki] notes/a | 2026-01-02 (score: 0.5)",
      "   second snippet",
    ].join("\n");
    const result = expectShape(formatToolResult(raw), "list");
    expect(result.label).toBe("Found 2 result(s):");
    expect(result.items).toEqual([
      "1. [observations] ep-1 | 2026-01-01 (score: 0.87)\n   first snippet",
      "2. [wiki] notes/a | 2026-01-02 (score: 0.5)\n   second snippet",
    ]);
  });

  it("does not treat ls-style dashes or loose numbered lines as a list", () => {
    expect(formatToolResult("-rw-r--r-- 1 user file.txt\n-rw-r--r-- 1 user other.txt").shape).toBe(
      "text",
    );
    expect(formatToolResult("Building target\n1. compiled foo\n2. compiled bar").shape).toBe(
      "text",
    );
  });

  it("marks a reported failure even when the text is ordinary", () => {
    const result = expectShape(
      formatToolResult("no agent key named 'gh'", { isError: true }),
      "text",
    );
    expect(result.error).toBe(true);
    expect(result.text).toBe("no agent key named 'gh'");
  });

  it("marks error-shaped text when history dropped the failure flag", () => {
    expect(formatToolResult("failed to read notes.txt: not found").error).toBe(true);
    expect(formatToolResult("command exited with code 1\nboom").error).toBe(true);
    expect(formatToolResult("search failed: database is locked").error).toBe(true);
    expect(formatToolResult("HTTP 404 fetching https://example.com").error).toBe(true);
  });

  it("does not mark a cancellation or a later mention of an error as a failure", () => {
    expect(formatToolResult("the turn was stopped while this command was running").error).toBe(
      false,
    );
    expect(formatToolResult("all good\nerror: this is just a log line").error).toBe(false);
  });

  it("marks a JSON object that is itself an error", () => {
    expect(formatToolResult('{"error":"nope"}').error).toBe(true);
    expect(formatToolResult('{"is_error":true,"message":"nope"}').error).toBe(true);
    expect(formatToolResult('{"error":"none","items":[1]}').error).toBe(false);
  });

  it("keeps a file's shape when the call failed", () => {
    const raw = "file: 4 bytes, 1 line(s) total\n\n   1\thi";
    const result = expectShape(formatToolResult(raw, { isError: true }), "file");
    expect(result.error).toBe(true);
  });

  it("collapses output past the line preview and reports how many lines are hidden", () => {
    const lines = Array.from(
      { length: TOOL_RESULT_PREVIEW_LINES + 3 },
      (_, index) => `line ${index}`,
    );
    const result = expectShape(formatToolResult(lines.join("\n")), "text");
    expect(result.collapse.long).toBe(true);
    expect(result.collapse.hiddenLines).toBe(3);
    expect(result.preview.split("\n")).toHaveLength(TOOL_RESULT_PREVIEW_LINES);
    expect(result.text.split("\n")).toHaveLength(TOOL_RESULT_PREVIEW_LINES + 3);
  });

  it("does not collapse output that fits in the preview", () => {
    const lines = Array.from({ length: TOOL_RESULT_PREVIEW_LINES }, () => "ok");
    const result = expectShape(formatToolResult(lines.join("\n")), "text");
    expect(result.collapse.long).toBe(false);
    expect(result.preview).toBe(result.text);
  });

  it("collapses a single very long line", () => {
    const text = "x".repeat(TOOL_RESULT_PREVIEW_CHARS + 40);
    const result = expectShape(formatToolResult(text), "text");
    expect(result.collapse.long).toBe(true);
    expect(result.collapse.hiddenLines).toBe(0);
    expect(result.preview.endsWith("…")).toBe(true);
    expect(result.preview.length).toBe(TOOL_RESULT_PREVIEW_CHARS + 1);
  });

  it("collapses a long file by source row, leaving the header out of the count", () => {
    const body = Array.from(
      { length: TOOL_RESULT_PREVIEW_LINES + 2 },
      (_, index) => `${String(index + 1).padStart(4, " ")}\tline`,
    );
    const raw = ["file: 100 bytes, 18 line(s) total", "", ...body].join("\n");
    const result = expectShape(formatToolResult(raw), "file");
    expect(result.collapse.long).toBe(true);
    expect(result.collapse.hiddenLines).toBe(2);
    expect(result.label.startsWith("file:")).toBe(true);
  });
});

describe("toolResultToggleLabel", () => {
  it("names how many lines are hidden", () => {
    expect(toolResultToggleLabel(1, false)).toBe("Show all (1 more line)");
    expect(toolResultToggleLabel(4, false)).toBe("Show all (4 more lines)");
    expect(toolResultToggleLabel(0, false)).toBe("Show all");
    expect(toolResultToggleLabel(4, true)).toBe("Show less");
  });
});
