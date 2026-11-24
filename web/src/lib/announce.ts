const EXCERPT_CHARS = 140;

export function replyExcerpt(markdown: string): string {
  const plain = markdown
    .replace(/```[\s\S]*?(?:```|$)/g, " code ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-*+]|\d+[.)])\s+/gm, "")
    .replace(/[*`~|]/g, "")
    .replace(/(?<![A-Za-z0-9])_+|_+(?![A-Za-z0-9])/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (plain.length <= EXCERPT_CHARS) return plain;
  const cut = plain.slice(0, EXCERPT_CHARS);
  const atWord = cut.lastIndexOf(" ");
  return `${(atWord > EXCERPT_CHARS / 2 ? cut.slice(0, atWord) : cut).replace(/[\s,;:.-]+$/, "")}…`;
}

export function workingAnnouncement(agent: string): string {
  return `${agent} is working`;
}

export function repliedAnnouncement(agent: string, reply: string): string {
  const excerpt = replyExcerpt(reply);
  return excerpt === "" ? `${agent} replied` : `${agent} replied: ${excerpt}`;
}

export function failedAnnouncement(agent: string): string {
  return `${agent} couldn't finish`;
}
