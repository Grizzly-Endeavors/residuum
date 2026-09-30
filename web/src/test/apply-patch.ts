import { parse, stringify } from "smol-toml";

function applyDiff(doc: Record<string, unknown>, diff: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(diff)) {
    if (value === null) {
      delete doc[key];
    } else if (typeof value === "object" && !Array.isArray(value)) {
      if ("$inline" in value) {
        doc[key] = value.$inline;
        continue;
      }
      const child = (doc[key] ?? {}) as Record<string, unknown>;
      applyDiff(child, value as Record<string, unknown>);
      doc[key] = child;
    } else {
      doc[key] = value;
    }
  }
}

/**
 * Apply a PATCH diff to a config file's text, the way the server does: `null`
 * removes a key, an object recurses into a table, `{$inline: {...}}` sets a
 * table, anything else sets the value. The result is the document written out
 * again.
 */
export function applyPatch(
  text: string,
  diff: Record<string, unknown>,
  format: "toml" | "json" = "toml",
): string {
  let doc: Record<string, unknown> = {};
  if (text.trim() !== "") {
    doc = (format === "json" ? JSON.parse(text) : parse(text)) as Record<string, unknown>;
  }
  applyDiff(doc, diff);
  return format === "json" ? JSON.stringify(doc) : stringify(doc);
}
