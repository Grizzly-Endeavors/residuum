import type { McpCatalogEntry, McpServerEntry } from "./types";

export function argsText(args: readonly string[]): string {
  return args.join("\n");
}

export function parseArgs(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

export function pairsText(pairs: Readonly<Record<string, string>>): string {
  return Object.entries(pairs)
    .map(([name, value]) => `${name}=${value}`)
    .join("\n");
}

export interface ParsedPairs {
  pairs: Record<string, string>;
  skipped: number[];
}

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

export function skippedLinesProblem(skipped: readonly number[]): string | undefined {
  if (skipped.length === 0) return undefined;
  const lines =
    skipped.length === 1 ? `Line ${String(skipped[0])} has` : `Lines ${skipped.join(", ")} have`;
  return `${lines} no NAME= before the value, so ${skipped.length === 1 ? "it is" : "they are"} left out.`;
}

export function catalogInputVariable(field: string): string {
  return field.startsWith("env.") ? field.slice(4) : field;
}

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

export function serverNameProblem(name: string, servers: readonly McpServerEntry[]): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "Give the server a name.";
  if (servers.some((server) => server.name === trimmed)) {
    return `There's already a server named ${trimmed}.`;
  }
  return null;
}
