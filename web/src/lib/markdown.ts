import { Marked, type Tokens } from "marked";
import DOMPurify from "dompurify";

const PATH_SEGMENT = /^(?!\.+$)[\p{L}\p{N}._-]+$/u;

export function isWorkspacePath(text: string): boolean {
  const segments = text.split("/");
  const last = segments.at(-1) ?? "";
  return (
    segments.length >= 2 &&
    last.includes(".") &&
    segments.every((segment) => PATH_SEGMENT.test(segment))
  );
}

export interface RelativeFileLinks {
  base: string;
  href: (path: string) => string;
}

export interface MarkdownOptions {
  pathHref?: (path: string) => string;
  relativeFileLinks?: RelativeFileLinks;
}

function isRelativeHref(href: string): boolean {
  return !/^([a-z][a-z0-9+.-]*:|\/|#)/i.test(href);
}

export function resolveRelativeFileLink(base: string, href: string): string | null {
  if (!isRelativeHref(href)) return null;
  let target: string;
  try {
    target = decodeURIComponent(href.split(/[?#]/)[0] ?? "");
  } catch {
    return null;
  }
  if (!/\.md$/i.test(target)) return null;
  const segments = base.split("/").slice(0, -1);
  for (const segment of target.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment !== "..") {
      segments.push(segment);
      continue;
    }
    if (segments.length === 0) return null;
    segments.pop();
  }
  return segments.length === 0 ? null : segments.join("/");
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const FORBIDDEN_TAGS = [
  "style",
  "link",
  "meta",
  "base",
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
  "iframe",
  "object",
  "embed",
  "svg",
  "math",
  "template",
  "dialog",
  "marquee",
  "video",
  "audio",
  "source",
  "track",
  "picture",
  "area",
  "map",
];

const FORBIDDEN_ATTRS = ["style", "popover", "srcset", "background", "poster"];

export const IMAGE_LINK_KIND = "Image";

const AGENT_FILE_PATH = /^\/api\/agents\/[^/]+\/files\/[^/]/;

function isShownImageSource(src: string): boolean {
  let url: URL;
  try {
    url = new URL(src, window.location.href);
  } catch {
    return false;
  }
  if (url.protocol === "data:") return true;
  return url.origin === window.location.origin && AGENT_FILE_PATH.test(url.pathname);
}

function linkUnshownImages(fragment: DocumentFragment): void {
  const doc = fragment.ownerDocument;

  for (const image of fragment.querySelectorAll("img")) {
    const src = image.getAttribute("src")?.trim() ?? "";
    if (src !== "" && isShownImageSource(src)) continue;

    const kind = doc.createElement("span");
    kind.className = "prose-image-link-kind";
    kind.textContent = IMAGE_LINK_KIND;
    const label = image.getAttribute("alt")?.trim() || src;

    const linked = src !== "" && image.closest("a") === null;
    const stand = doc.createElement(linked ? "a" : "span");
    stand.className = "prose-image-link";
    if (linked) stand.setAttribute("href", src);
    stand.append(kind);
    if (label !== "") stand.append(` ${label}`);
    image.replaceWith(stand);
  }
}

const HEADING_DEMOTION = 2;
const DEEPEST_HEADING = 6;

const TASK_DONE = "Done: ";
const TASK_TO_DO = "To do: ";

export const COPY_LABEL = "Copy";
export const COPY_NAME = "Copy code";

function fitToPage(fragment: DocumentFragment): void {
  const doc = fragment.ownerDocument;

  linkUnshownImages(fragment);

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
    if (link.hasAttribute("data-path") || link.getAttribute("href")?.startsWith("#") === true) {
      continue;
    }
    link.setAttribute("target", "_blank");
    link.setAttribute("rel", "noopener noreferrer");
  }
}

export function renderMarkdown(content: string, options: MarkdownOptions = {}): DocumentFragment {
  const { pathHref, relativeFileLinks } = options;
  const marked = new Marked({
    gfm: true,
    breaks: true,
    renderer: {
      code({ text, lang }: Tokens.Code): string {
        const language = /^\S+/.exec(lang ?? "")?.[0];
        const langClass = language ? ` class="language-${escapeHtml(language)}"` : "";
        const label = language ? `Code, ${escapeHtml(language)}` : "Code";
        return `<div class="prose-code"><pre tabindex="0" role="group" aria-label="${label}"><code${langClass}>${escapeHtml(text)}\n</code></pre></div>\n`;
      },
      link({ href, title, tokens }: Tokens.Link): string {
        const text = this.parser.parseInline(tokens);
        const path =
          relativeFileLinks === undefined
            ? null
            : resolveRelativeFileLink(relativeFileLinks.base, href);
        if (relativeFileLinks !== undefined && path !== null) {
          return `<a href="${escapeHtml(relativeFileLinks.href(path))}" data-path="${escapeHtml(path)}">${text}</a>`;
        }
        const titled = title ? ` title="${escapeHtml(title)}"` : "";
        return `<a href="${escapeHtml(href)}"${titled}>${text}</a>`;
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
  }) as DocumentFragment | null;
  if (fragment === null) {
    const text = document.createDocumentFragment();
    text.append(content);
    return text;
  }
  fitToPage(fragment);
  return fragment;
}
