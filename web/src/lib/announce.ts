// What a screen reader is told about the agent's work, in short sentences
// that stand on their own.

/** How much of a reply is read out before the rest is left to the conversation itself. */
const EXCERPT_CHARS = 140;

/**
 * The start of a reply as plain words: Markdown marks, code blocks and link
 * addresses dropped, cut at a word, with an ellipsis when there was more.
 * Empty when nothing readable is left.
 */
export function replyExcerpt(markdown: string): string {
  const plain = markdown
    .replace(/```[\s\S]*?(?:```|$)/g, " code ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(?:#{1,6}|>|[-*+]|\d+[.)])\s+/gm, "")
    .replace(/[*`~|]/g, "")
    // Emphasis underscores, not the ones inside a name like memory_search.
    .replace(/(?<![A-Za-z0-9])_+|_+(?![A-Za-z0-9])/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (plain.length <= EXCERPT_CHARS) return plain;
  const cut = plain.slice(0, EXCERPT_CHARS);
  const atWord = cut.lastIndexOf(" ");
  return `${(atWord > EXCERPT_CHARS / 2 ? cut.slice(0, atWord) : cut).replace(/[\s,;:.-]+$/, "")}…`;
}

/** What is said when the agent starts a turn. */
export function workingAnnouncement(agent: string): string {
  return `${agent} is working`;
}

/** What is said when the agent's reply is complete, with the start of it. */
export function repliedAnnouncement(agent: string, reply: string): string {
  const excerpt = replyExcerpt(reply);
  return excerpt === "" ? `${agent} replied` : `${agent} replied: ${excerpt}`;
}

/** What is said when a turn couldn't finish. */
export function failedAnnouncement(agent: string): string {
  return `${agent} couldn't finish`;
}
