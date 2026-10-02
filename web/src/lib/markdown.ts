import { Marked, type Tokens } from "marked";
import DOMPurify from "dompurify";

/** One path segment: letters, digits, `.`, `_` or `-`, and not only dots. */
const PATH_SEGMENT = /^(?!\.+$)[\p{L}\p{N}._-]+$/u;

/**
 * Whether `text` is a whole workspace path: two or more `/`-separated
 * segments of letters, digits, `.`, `_` or `-`, the last with a `.` in it.
 * `team/wiki/index.md` is one; `team/wiki`, `/etc/hosts` and `../a.md` aren't.
 */
export function isWorkspacePath(text: string): boolean {
  const segments = text.split("/");
  const last = segments.at(-1) ?? "";
  return (
    segments.length >= 2 &&
    last.includes(".") &&
    segments.every((segment) => PATH_SEGMENT.test(segment))
  );
}

export interface MarkdownOptions {
  /**
   * The link for inline code whose whole text is a workspace path. Without
   * it, paths stay plain code.
   */
  pathHref?: (path: string) => string;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/**
 * Render message Markdown (GFM, single line breaks kept) to sanitized DOM
 * nodes: the sanitizer parses the HTML, so no unsanitized string reaches the
 * page. Each code block carries a `data-copy-code` button, and a workspace
 * path in inline code becomes an `a[data-path]` when `pathHref` is given; the
 * view showing the nodes handles their clicks.
 */
export function renderMarkdown(content: string, options: MarkdownOptions = {}): DocumentFragment {
  const { pathHref } = options;
  const marked = new Marked({
    gfm: true,
    breaks: true,
    renderer: {
      code({ text, lang }: Tokens.Code): string {
        const language = /^\S+/.exec(lang ?? "")?.[0];
        const langClass = language ? ` class="language-${escapeHtml(language)}"` : "";
        return (
          `<div class="prose-code"><pre><code${langClass}>${escapeHtml(text)}\n</code></pre>` +
          `<button type="button" class="prose-copy" data-copy-code>Copy</button></div>\n`
        );
      },
      codespan({ text }: Tokens.Codespan): string {
        const code = `<code>${escapeHtml(text)}</code>`;
        if (pathHref === undefined || !isWorkspacePath(text)) return code;
        return `<a class="prose-path" href="${escapeHtml(pathHref(text))}" data-path="${escapeHtml(text)}">${code}</a>`;
      },
    },
  });
  const rawHtml = marked.parse(content, { async: false });
  return DOMPurify.sanitize(rawHtml, { USE_PROFILES: { html: true }, RETURN_DOM_FRAGMENT: true });
}
