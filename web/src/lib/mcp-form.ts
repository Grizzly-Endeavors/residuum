// The text forms the Tool servers section edits a server's arguments,
// environment and headers in, and a catalog entry made into a server.
// Arguments go one per line, so an argument with a space in it survives an
// edit; environment variables and headers go one `NAME=value` per line.

import type { McpCatalogEntry, McpServerEntry } from "./types";

/** A server's arguments, one per line. */
export function argsText(args: readonly string[]): string {
  return args.join("\n");
}

/** Arguments from their lines, leaving out blank ones. */
export function parseArgs(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

/** Environment variables or headers, one `NAME=value` per line. */
export function pairsText(pairs: Readonly<Record<string, string>>): string {
  return Object.entries(pairs)
    .map(([name, value]) => `${name}=${value}`)
    .join("\n");
}

export interface ParsedPairs {
  pairs: Record<string, string>;
  /** The 1-based numbers of lines that aren't blank and have no name before an `=`, which are left out. */
  skipped: number[];
}

/** `NAME=value` lines as pairs, noting the lines that can't be read as one. */
export function parsePairs(text: string): ParsedPairs {
  const pairs: Record<string, string> = {};
  const skipped: number[] = [];
  text.split("\n").forEach((line, index) => {
    if (line.trim() === "") return;
    const eq = line.indexOf("=");
    const name = eq > 0 ? line.slice(0, eq).trim() : "";
    if (name === "") skipped.push(index + 1);
    else pairs[name] = line.slice(eq + 1).trim();
  });
  return { pairs, skipped };
}

/** Why some lines were left out, for the field's error; undefined when none were. */
export function skippedLinesProblem(skipped: readonly number[]): string | undefined {
  if (skipped.length === 0) return undefined;
  const lines =
    skipped.length === 1 ? `Line ${String(skipped[0])} has` : `Lines ${skipped.join(", ")} have`;
  return `${lines} no NAME= before the value, so ${skipped.length === 1 ? "it is" : "they are"} left out.`;
}

/** The environment variable a catalog input fills: `env.GITHUB_TOKEN` fills `GITHUB_TOKEN`. */
export function catalogInputVariable(field: string): string {
  return field.startsWith("env.") ? field.slice(4) : field;
}

/** A catalog entry as a server to add, with what was typed for its inputs. */
export function serverFromCatalog(
  entry: McpCatalogEntry,
  inputs: Readonly<Record<string, string>>,
): McpServerEntry {
  const env = { ...entry.env };
  for (const input of entry.requires_input) {
    env[catalogInputVariable(input.field)] = (inputs[input.field] ?? "").trim();
  }
  return {
    name: entry.name,
    transport: "stdio",
    command: entry.command,
    args: [...entry.args],
    env,
  };
}

/** Why a new server can't take `name`, or null when it can. */
export function serverNameProblem(name: string, servers: readonly McpServerEntry[]): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "Give the server a name.";
  if (servers.some((server) => server.name === trimmed)) {
    return `There's already a server named ${trimmed}.`;
  }
  return null;
}
