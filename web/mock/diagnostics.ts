import { parse as parseToml } from "smol-toml";
import type { Diagnostic, ValidateResponse } from "../src/lib/types";

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
    return [{ severity: "error", message: message.split("\n", 1)[0] ?? message, location }];
  }
}

export function validation(
  format: "toml" | "json",
  content: string,
  check: (text: string) => Diagnostic[] = () => [],
): ValidateResponse {
  const syntax = diagnoseConfigText(format, content);
  const diagnostics = syntax.length > 0 ? syntax : check(content);
  const first = diagnostics[0];
  return first === undefined
    ? { valid: true }
    : { valid: false, error: first.message, diagnostics };
}

export function configFormatOf(path: string): "toml" | "json" | null {
  if (/(^|\/)(config|providers|channels)\.toml$/.test(path)) return "toml";
  if (/(^|\/)config\/(mcp|a2a)\.json$/.test(path)) return "json";
  return null;
}
