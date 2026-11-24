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
      const existing = doc[key];
      const child =
        typeof existing === "object" && existing !== null && !Array.isArray(existing)
          ? (existing as Record<string, unknown>)
          : {};
      applyDiff(child, value as Record<string, unknown>);
      doc[key] = child;
    } else {
      doc[key] = value;
    }
  }
}

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
