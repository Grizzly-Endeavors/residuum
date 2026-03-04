// ── Markdown-to-HTML renderer ────────────────────────────────────────
// Port of Chat.renderMarkdown() from assets/web/chat.js

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

export function renderMarkdown(text: string): string {
  let html = escapeHtml(text);

  // Code blocks: ```lang\n...\n```
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_m, _lang, code) => {
    return `<pre><code>${code.trim()}</code></pre>`;
  });

  // Inline code: `...`
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");

  // Bold: **...**
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");

  // Italic: *...*
  html = html.replace(
    /(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g,
    "<em>$1</em>",
  );

  // Links: [text](url)
  html = html.replace(
    /\[([^\]]+)\]\(([^)]+)\)/g,
    '<a href="$2" target="_blank" rel="noopener">$1</a>',
  );

  // Horizontal rules: --- or *** on their own line
  html = html.replace(/\n(---|\*\*\*)\n/g, "\n<hr>\n");

  // Headings: ## through ####
  html = html.replace(/^#### (.+)$/gm, "<h4>$1</h4>");
  html = html.replace(/^### (.+)$/gm, "<h3>$1</h3>");
  html = html.replace(/^## (.+)$/gm, "<h2>$1</h2>");

  // Blockquotes: > text
  html = html.replace(/^&gt; (.+)$/gm, "<blockquote>$1</blockquote>");
  html = html.replace(/<\/blockquote>\n<blockquote>/g, "<br>");

  // Unordered lists: - item
  html = html.replace(/(^|\n)(- .+(?:\n- .+)*)/g, (_m, pre, block) => {
    const items = block
      .split("\n")
      .map((line: string) => `<li>${line.replace(/^- /, "")}</li>`)
      .join("");
    return `${pre}<ul>${items}</ul>`;
  });

  // Ordered lists: 1. item
  html = html.replace(
    /(^|\n)(\d+\. .+(?:\n\d+\. .+)*)/g,
    (_m, pre, block) => {
      const items = block
        .split("\n")
        .map((line: string) => `<li>${line.replace(/^\d+\. /, "")}</li>`)
        .join("");
      return `${pre}<ol>${items}</ol>`;
    },
  );

  // Line breaks (outside of pre blocks)
  html = html.replace(/\n/g, "<br>");

  // Fix line breaks inside pre that got doubled
  html = html.replace(
    /<pre><code>([\s\S]*?)<\/code><\/pre>/g,
    (_m, inner) => {
      return `<pre><code>${inner.replace(/<br>/g, "\n")}</code></pre>`;
    },
  );

  // Clean up line breaks adjacent to block elements
  html = html.replace(
    /<br>(<\/?(?:h[234]|blockquote|ul|ol|li|hr|pre)>)/g,
    "$1",
  );
  html = html.replace(
    /(<\/?(?:h[234]|blockquote|ul|ol|li|hr|pre)>)<br>/g,
    "$1",
  );

  return html;
}
