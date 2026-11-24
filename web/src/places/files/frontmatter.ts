export interface FrontmatterField {
  key: string;
  value: string;
}

export interface SplitFrontmatter {
  fields: FrontmatterField[];
  body: string;
}

const FRONTMATTER = /^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/;
const KEY_LINE = /^([A-Za-z_][\w-]*):(?:[ \t]+(.*))?$/;

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
