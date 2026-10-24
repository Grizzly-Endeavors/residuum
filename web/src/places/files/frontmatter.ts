// A Markdown file's leading YAML frontmatter, split from its body so it can
// show as a key/value block instead of rendering as a rule and a heading.

export interface FrontmatterField {
  key: string;
  /** The value as written, with any indented or list lines after its key line. */
  value: string;
}

export interface SplitFrontmatter {
  fields: FrontmatterField[];
  body: string;
}

const FRONTMATTER = /^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/;
const KEY_LINE = /^([A-Za-z_][\w-]*):(?:[ \t]+(.*))?$/;

/**
 * The frontmatter block at the top of `content`, with its fields, and the
 * body after it; null when the file has no frontmatter. Lines that aren't a
 * `key:` line continue the field above them, and a comment line is dropped.
 */
export function splitFrontmatter(content: string): SplitFrontmatter | null {
  const match = FRONTMATTER.exec(content);
  if (match === null) return null;
  const fields: FrontmatterField[] = [];
  for (const line of (match[1] ?? "").split(/\r?\n/)) {
    if (line.trimStart().startsWith("#")) continue;
    const key = KEY_LINE.exec(line);
    if (key !== null) {
      fields.push({ key: key[1] ?? "", value: (key[2] ?? "").trim() });
      continue;
    }
    const last = fields.at(-1);
    if (last === undefined) return null;
    last.value = last.value === "" ? line.trim() : `${last.value}\n${line.trim()}`;
  }
  if (fields.length === 0) return null;
  return { fields, body: content.slice(match[0].length) };
}
