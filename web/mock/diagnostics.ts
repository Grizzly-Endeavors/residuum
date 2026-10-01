import { parse as parseToml } from "smol-toml";
import type { Diagnostic, ValidateResponse } from "../src/lib/types";

/**
 * Problems in a config file's text, the way the backend's validators report
 * them: a syntax error at its line and column. The mock has no semantic
 * checks, so text that parses has none.
 */
export function diagnoseConfigText(format: "toml" | "json", content: string): Diagnostic[] {
  if (content.trim() === "") return [];
  try {
    if (format === "toml") parseToml(content);
    else JSON.parse(content);
    return [];
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const at = /line (\d+) column (\d+)/.exec(message);
    const { line = Number(at?.[1]), column = Number(at?.[2]) } = err as {
      line?: number;
      column?: number;
    };
    const location =
      line > 0 && column > 0 ? { kind: "line_column" as const, line, column } : undefined;
    const first = message.split("\n", 1)[0] ?? message;
    return [
      { severity: "error", message: first.replace(/^Invalid TOML document: /, ""), location },
    ];
  }
}

/** A validate or raw-save answer for `content`, as `ValidateResponse::from_diagnostics` builds it. */
export function validation(format: "toml" | "json", content: string): ValidateResponse {
  const diagnostics = diagnoseConfigText(format, content);
  const first = diagnostics[0];
  return first === undefined
    ? { valid: true }
    : { valid: false, error: first.message, diagnostics };
}

/** The format of a workspace path the validate route checks, or null for one it doesn't. */
export function configFormatOf(path: string): "toml" | "json" | null {
  if (/(^|\/)(config|providers|channels)\.toml$/.test(path)) return "toml";
  if (/(^|\/)config\/(mcp|a2a)\.json$/.test(path)) return "json";
  return null;
}
