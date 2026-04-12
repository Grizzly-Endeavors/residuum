import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ gfm: true, breaks: true });

export function renderMarkdown(content: string): string {
  const rawHtml = marked.parse(content, { async: false });
  return DOMPurify.sanitize(rawHtml, { USE_PROFILES: { html: true } });
}
