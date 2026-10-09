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

/** How far a reply's headings sit below the page's own: a reply's `#` is an `h3`. */
const HEADING_DEMOTION = 2;
const DEEPEST_HEADING = 6;

/** What a task item's glyph says to assistive technology, which can't see it. */
const TASK_DONE = "Done: ";
const TASK_TO_DO = "To do: ";

/**
 * What a reply's HTML needs after the sanitizer and before the page: headings
 * moved below the page's own, each table inside a scroller the keyboard can
 * reach, and links that open beside the app rather than over it. These are
 * DOM changes, not Markdown renderers, because they have to hold for the raw
 * HTML a reply can carry as well, and the sanitizer drops `target`.
 */
function fitToPage(fragment: DocumentFragment): void {
  const doc = fragment.ownerDocument;

  for (const heading of fragment.querySelectorAll("h1, h2, h3, h4, h5, h6")) {
    const level = Math.min(Number(heading.tagName.slice(1)) + HEADING_DEMOTION, DEEPEST_HEADING);
    const demoted = doc.createElement(`h${String(level)}`);
    for (const { name, value } of heading.attributes) demoted.setAttribute(name, value);
    demoted.append(...heading.childNodes);
    heading.replaceWith(demoted);
  }

  for (const table of fragment.querySelectorAll("table")) {
    const scroller = doc.createElement("div");
    scroller.className = "prose-table";
    scroller.setAttribute("role", "group");
    scroller.setAttribute("aria-label", "Table");
    scroller.tabIndex = 0;
    table.replaceWith(scroller);
    scroller.append(table);
  }

  for (const link of fragment.querySelectorAll("a[href]")) {
    // A workspace path opens in the app's panel, and a fragment stays on the page.
    if (link.hasAttribute("data-path") || link.getAttribute("href")?.startsWith("#") === true) {
      continue;
    }
    link.setAttribute("target", "_blank");
    link.setAttribute("rel", "noopener noreferrer");
  }
}

/**
 * Render message Markdown (GFM, single line breaks kept) to sanitized DOM
 * nodes: the sanitizer parses the HTML, so no unsanitized string reaches the
 * page. Each code block carries a `data-copy-code` button, and a workspace
 * path in inline code becomes an `a[data-path]` when `pathHref` is given; the
 * view showing the nodes handles their clicks. A task list item leads with a
 * `.prose-check` that names its state to assistive technology. Headings sit
 * two levels below the page's own, tables scroll in a labelled group the
 * keyboard can focus, and links outside the app open in a new tab.
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
        const label = language ? `Code, ${escapeHtml(language)}` : "Code";
        // The block scrolls sideways, so the keyboard has to be able to reach it.
        return (
          `<div class="prose-code"><pre tabindex="0" role="group" aria-label="${label}"><code${langClass}>${escapeHtml(text)}\n</code></pre>` +
          `<button type="button" class="prose-copy" aria-label="Copy code" data-copy-code>Copy</button></div>\n`
        );
      },
      codespan({ text }: Tokens.Codespan): string {
        const code = `<code>${escapeHtml(text)}</code>`;
        if (pathHref === undefined || !isWorkspacePath(text)) return code;
        return `<a class="prose-path" href="${escapeHtml(pathHref(text))}" data-path="${escapeHtml(text)}">${code}</a>`;
      },
      listitem(item: Tokens.ListItem): string | false {
        if (!item.task) return false;
        return `<li class="prose-task">${this.parser.parse(item.tokens)}</li>\n`;
      },
      checkbox({ checked }: Tokens.Checkbox): string {
        const state = checked ? TASK_DONE : TASK_TO_DO;
        return `<span class="prose-check" data-done="${String(checked)}"><span class="prose-sr-only">${state}</span></span>`;
      },
    },
  });
  const rawHtml = marked.parse(content, { async: false });
  const fragment = DOMPurify.sanitize(rawHtml, {
    USE_PROFILES: { html: true },
    RETURN_DOM_FRAGMENT: true,
  });
  fitToPage(fragment);
  return fragment;
}
