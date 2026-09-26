// ── Tool result formatting ────────────────────────────────────────────
//
// Tool output reaches the feed as one string (see `appendResult`). This
// module turns that string into something the tool row can render: JSON
// indented, a numbered file dump split into a gutter, a list broken into
// items, and a failure drawn differently from ordinary output.
//
// Anything that isn't clearly one of those shapes is returned unchanged.
// A parse that doesn't fit falls back to the original text, so a result
// is never dropped or replaced with an error of our own.

import { TOOL_RESULT_MARKER } from "./feed-items";

/** Lines kept visible before a result offers to expand. */
export const TOOL_RESULT_PREVIEW_LINES = 16;

/**
 * Character count that collapses a result made of few but very long lines.
 * Line count alone would leave a single minified payload fully open.
 */
export const TOOL_RESULT_PREVIEW_CHARS = 1_200;

export interface ToolResultCollapse {
  /** Whether the result should start collapsed. */
  long: boolean;
  /**
   * Lines or list items omitted from the collapsed view.
   * Zero when the result is collapsed for length rather than line count.
   */
  hiddenLines: number;
}

/** One row of a numbered file dump (`read_file`, or an `edit_file` preview). */
export type FileRow =
  | { kind: "code"; number: string; text: string }
  | { kind: "gap"; text: string };

interface ToolResultBase {
  /** The call failed, or the text itself reads as a failure. */
  error: boolean;
  /**
   * Prose kept visible above the body while the body is collapsed:
   * a file header, a list intro, or a fetch preamble ahead of JSON.
   */
  label: string;
  collapse: ToolResultCollapse;
}

export interface JsonToolResult extends ToolResultBase {
  shape: "json";
  text: string;
  preview: string;
}

export interface FileToolResult extends ToolResultBase {
  shape: "file";
  rows: readonly FileRow[];
}

export interface ListToolResult extends ToolResultBase {
  shape: "list";
  items: readonly string[];
}

export interface TextToolResult extends ToolResultBase {
  shape: "text";
  text: string;
  preview: string;
}

export type FormattedToolResult = JsonToolResult | FileToolResult | ListToolResult | TextToolResult;

export interface FormatToolResultOptions {
  /**
   * The call was reported as a failure. History does not keep that flag,
   * so a reloaded transcript also checks the text itself.
   */
  isError?: boolean;
}

/**
 * Leading phrases built-in tools use when a call fails. Matched only at
 * the start of the result so a later mention of "error" in ordinary
 * output (a file, a log) stays ordinary output.
 */
const ERROR_START =
  /^(?:error:|error$|failed to\b|command exited with code \d+\b|command timed out\b|HTTP [45]\d{2}\b|search failed:)/i;

const FILE_HEADER = /^file: \d+ bytes, \d+ line\(s\) total$/;
const NUMBERED_LINE = /^(\s*)(\d+)\t(.*)$/;
const GAP_LINE = /^\s*…/;
const ITEM_START = /^(?:[-*•]|\d+[.)])\s+\S/;

/** Label for the expand control under a long result. */
export function toolResultToggleLabel(hiddenLines: number, expanded: boolean): string {
  if (expanded) return "Show less";
  if (hiddenLines === 1) return "Show all (1 more line)";
  if (hiddenLines > 1) return `Show all (${hiddenLines} more lines)`;
  return "Show all";
}

/**
 * Format one stored tool result for display.
 *
 * `raw` may include the feed's `TOOL_RESULT_MARKER` prefixes. Those are
 * storage, not output, and are removed. Several chunks are formatted on
 * their own and then shown as one block.
 */
export function formatToolResult(
  raw: string,
  options?: FormatToolResultOptions,
): FormattedToolResult {
  const isError = options?.isError === true;
  const chunks = resultChunks(raw);
  if (chunks.length === 0) return textResult("", isError);

  const formatted = chunks.map(formatChunk);
  const [only] = formatted;
  if (formatted.length === 1 && only) {
    return isError && !only.error ? { ...only, error: true } : only;
  }
  return mergeChunks(formatted, isError);
}

function resultChunks(raw: string): string[] {
  const normalized = raw.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const marker = TOOL_RESULT_MARKER.replace(/\n$/, "");
  if (!normalized.includes(marker)) {
    const trimmed = normalized.trim();
    return trimmed === "" ? [] : [trimmed];
  }

  const parts = normalized.split(marker);
  const bodies: string[] = [];
  const leading = parts[0]?.trim() ?? "";
  if (leading !== "") bodies.push(leading);
  for (const part of parts.slice(1)) {
    const body = part.replace(/^\n/, "").trim();
    if (body !== "") bodies.push(body);
  }
  return bodies;
}

function formatChunk(chunk: string): FormattedToolResult {
  const file = parseFile(chunk);
  if (file) return file;

  const json = parseJson(chunk);
  if (json) return json;

  const list = parseList(chunk);
  if (list) return list;

  const collapsed = collapseText(chunk);
  return {
    shape: "text",
    label: "",
    text: chunk,
    preview: collapsed.preview,
    error: textLooksLikeError(chunk),
    collapse: collapsed.collapse,
  };
}

function textResult(text: string, error: boolean): TextToolResult {
  return {
    shape: "text",
    label: "",
    text,
    preview: text,
    error,
    collapse: { long: false, hiddenLines: 0 },
  };
}

function parseFile(chunk: string): FileToolResult | null {
  const preamble: string[] = [];
  const rows: FileRow[] = [];
  let inBody = false;

  for (const line of chunk.split("\n")) {
    if (line.trim() === "") continue;

    const numbered = NUMBERED_LINE.exec(line);
    if (numbered) {
      inBody = true;
      const number = numbered[2];
      const text = numbered[3];
      if (number === undefined || text === undefined) continue;
      rows.push({ kind: "code", number, text });
      continue;
    }

    if (inBody) {
      if (GAP_LINE.test(line)) {
        rows.push({ kind: "gap", text: line.trim() });
        continue;
      }
      return null;
    }

    preamble.push(line);
  }

  const codeRows = rows.filter((row) => row.kind === "code").length;
  const header = preamble[0] ?? "";
  const headed = FILE_HEADER.test(header);
  if (headed) {
    if (codeRows === 0) return null;
  } else if (codeRows < 2) {
    return null;
  }

  const label = preamble.join("\n");
  const charLength = rows.reduce((sum, row) => sum + row.text.length, 0);
  return {
    shape: "file",
    label,
    rows,
    error: textLooksLikeError(chunk),
    collapse: collapseCount(rows.length, charLength),
  };
}

function parseJson(chunk: string): JsonToolResult | null {
  const direct = tryStructuredJson(chunk);
  if (direct) return jsonResult("", direct, chunk);

  const lines = chunk.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const start = lines[index]?.trimStart() ?? "";
    if (!start.startsWith("{") && !start.startsWith("[")) continue;
    const suffix = lines.slice(index).join("\n");
    const value = tryStructuredJson(suffix);
    if (!value) return null;
    const preamble = lines.slice(0, index).join("\n").trim();
    return jsonResult(preamble, value, chunk);
  }
  return null;
}

function jsonResult(label: string, value: object, chunk: string): JsonToolResult {
  const text = JSON.stringify(value, null, 2);
  const collapsed = collapseText(text);
  return {
    shape: "json",
    label,
    text,
    preview: collapsed.preview,
    error: textLooksLikeError(chunk) || jsonLooksLikeError(value),
    collapse: collapsed.collapse,
  };
}

function tryStructuredJson(text: string): object | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return null;
  try {
    const value: unknown = JSON.parse(trimmed);
    if (typeof value === "object" && value !== null) return value;
    return null;
  } catch {
    return null;
  }
}

/**
 * An object that is itself the failure, not a payload that happens to
 * mention an error field next to other data.
 */
function jsonLooksLikeError(value: object): boolean {
  if (Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  if (record["is_error"] === true) return true;
  const keys = Object.keys(record);
  const onlyErrorFields = keys.every(
    (key) => key === "error" || key === "message" || key === "is_error",
  );
  if (!onlyErrorFields) return false;
  const errorField = record["error"];
  return typeof errorField === "string" && errorField.trim() !== "";
}

function parseList(chunk: string): ListToolResult | null {
  const items: string[][] = [];
  const labelLines: string[] = [];
  let started = false;

  for (const line of chunk.split("\n")) {
    if (line.trim() === "") continue;

    if (ITEM_START.test(line)) {
      started = true;
      items.push([line]);
      continue;
    }

    if (!started) {
      if (labelLines.length > 0 || !line.trim().endsWith(":")) return null;
      labelLines.push(line.trim());
      continue;
    }

    if (/^[ \t]+\S/.test(line)) {
      const current = items[items.length - 1];
      if (!current) return null;
      current.push(line);
      continue;
    }

    return null;
  }

  if (items.length < 2) return null;
  const rendered = items.map((parts) => parts.join("\n"));
  const charLength = rendered.reduce((sum, item) => sum + item.length, 0);
  return {
    shape: "list",
    label: labelLines.join("\n"),
    items: rendered,
    error: textLooksLikeError(chunk),
    collapse: collapseCount(rendered.length, charLength),
  };
}

function textLooksLikeError(text: string): boolean {
  const first = text.split("\n")[0]?.trim() ?? "";
  return ERROR_START.test(first);
}

function collapseText(text: string): { preview: string; collapse: ToolResultCollapse } {
  const lines = text.split("\n");
  if (lines.length > TOOL_RESULT_PREVIEW_LINES) {
    return {
      preview: lines.slice(0, TOOL_RESULT_PREVIEW_LINES).join("\n"),
      collapse: {
        long: true,
        hiddenLines: lines.length - TOOL_RESULT_PREVIEW_LINES,
      },
    };
  }
  if (text.length > TOOL_RESULT_PREVIEW_CHARS) {
    return {
      preview: `${text.slice(0, TOOL_RESULT_PREVIEW_CHARS)}…`,
      collapse: { long: true, hiddenLines: 0 },
    };
  }
  return { preview: text, collapse: { long: false, hiddenLines: 0 } };
}

function collapseCount(count: number, charLength: number): ToolResultCollapse {
  if (count > TOOL_RESULT_PREVIEW_LINES) {
    return { long: true, hiddenLines: count - TOOL_RESULT_PREVIEW_LINES };
  }
  if (charLength > TOOL_RESULT_PREVIEW_CHARS) {
    return { long: true, hiddenLines: 0 };
  }
  return { long: false, hiddenLines: 0 };
}

function mergeChunks(chunks: readonly FormattedToolResult[], isError: boolean): TextToolResult {
  const text = chunks.map(chunkAsText).join("\n\n");
  const collapsed = collapseText(text);
  return {
    shape: "text",
    label: "",
    text,
    preview: collapsed.preview,
    error: isError || chunks.some((chunk) => chunk.error),
    collapse: collapsed.collapse,
  };
}

function chunkAsText(chunk: FormattedToolResult): string {
  let body: string;
  switch (chunk.shape) {
    case "json":
    case "text":
      body = chunk.text;
      break;
    case "file":
      body = chunk.rows
        .map((row) => (row.kind === "code" ? `${row.number}\t${row.text}` : row.text))
        .join("\n");
      break;
    case "list":
      body = chunk.items.join("\n\n");
      break;
  }
  if (chunk.label === "") return body;
  if (body === "") return chunk.label;
  return `${chunk.label}\n\n${body}`;
}
