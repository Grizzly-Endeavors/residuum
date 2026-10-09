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
 * What a reply's HTML may not carry. A reply is text a model wrote, often
 * after reading the web, so markup that restyles or covers the app, shows a
 * form that posts elsewhere, or loads a document, frame or script has no
 * place in it. The sanitizer's profile already drops several of these; they
 * are named here as well so a change of profile can't bring one back. A
 * removed element leaves its text behind, except `style`, `script` and the
 * like, whose contents the sanitizer drops with them.
 */
const FORBIDDEN_TAGS = [
  // Restyle the whole page from inside a message.
  "style",
  "link",
  "meta",
  "base",
  // Forms: a reply can't ask for input or post it anywhere.
  "form",
  "input",
  "button",
  "select",
  "option",
  "optgroup",
  "datalist",
  "textarea",
  "fieldset",
  "legend",
  "label",
  "output",
  // Documents, frames and plug-ins.
  "iframe",
  "object",
  "embed",
  "svg",
  "math",
  // Content placed over the page or moving on its own.
  "dialog",
  "marquee",
];

/**
 * Attributes dropped from every element. Inline `style` can position content
 * over the app (`position: fixed`), so no inline style is kept: a reply is
 * styled by the message's own stylesheet, never its own. `popover` makes an
 * element a top-layer overlay.
 */
const FORBIDDEN_ATTRS = ["style", "popover"];

/** How far a reply's headings sit below the page's own: a reply's `#` is an `h3`. */
const HEADING_DEMOTION = 2;
const DEEPEST_HEADING = 6;

/** What a task item's glyph says to assistive technology, which can't see it. */
const TASK_DONE = "Done: ";
const TASK_TO_DO = "To do: ";

/** A code block's Copy button: what it shows, and what assistive technology calls it. */
export const COPY_LABEL = "Copy";
export const COPY_NAME = "Copy code";

/**
 * What a reply's HTML needs after the sanitizer and before the page: headings
 * moved below the page's own, each table inside a scroller the keyboard can
 * reach, links that open beside the app rather than over it, and a Copy button
 * on each code block. These are DOM changes, not Markdown renderers, because
 * they have to hold for the raw HTML a reply can carry as well, the sanitizer
 * drops `target`, and it drops buttons: the Copy button is made here, after it.
 */
function fitToPage(fragment: DocumentFragment): void {
  const doc = fragment.ownerDocument;

  for (const block of fragment.querySelectorAll(".prose-code")) {
    const copy = doc.createElement("button");
    copy.type = "button";
    copy.className = "prose-copy";
    copy.setAttribute("aria-label", COPY_NAME);
    copy.setAttribute("data-copy-code", "");
    copy.textContent = COPY_LABEL;
    block.append(copy);
  }

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
 * keyboard can focus, and links outside the app open in a new tab. Raw HTML
 * in the reply keeps its text but loses styles, forms, frames and the other
 * elements and attributes in `FORBIDDEN_TAGS` and `FORBIDDEN_ATTRS`.
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
        return `<div class="prose-code"><pre tabindex="0" role="group" aria-label="${label}"><code${langClass}>${escapeHtml(text)}\n</code></pre></div>\n`;
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
    FORBID_TAGS: FORBIDDEN_TAGS,
    FORBID_ATTR: FORBIDDEN_ATTRS,
    RETURN_DOM_FRAGMENT: true,
  });
  fitToPage(fragment);
  return fragment;
}
