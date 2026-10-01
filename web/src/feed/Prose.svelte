<script lang="ts">
  import { renderMarkdown } from "../lib/markdown";
  import { openPathInPanel, pathHref } from "./feed-links";

  // Message text as sanitized Markdown. Each code block has a Copy button,
  // and inline code that is a whole workspace path opens that file in the
  // context panel, in `agent`'s workspace.

  interface Props {
    content: string;
    /** The agent whose workspace the message's paths are in. */
    agent: string;
    /** `compact` for text inside a card. */
    size?: "message" | "compact";
  }

  let { content, agent, size = "message" }: Props = $props();

  // The sanitizer builds the nodes; they replace what was shown whenever the
  // text or its agent changes.
  function render(node: HTMLElement): void {
    node.replaceChildren(renderMarkdown(content, { pathHref: (path) => pathHref(agent, path) }));
  }

  const COPY_LABEL = "Copy";
  const COPY_FEEDBACK_MS = 2000;

  async function copyCode(button: HTMLButtonElement): Promise<void> {
    const code = button.parentElement?.querySelector("code")?.textContent ?? "";
    try {
      await navigator.clipboard.writeText(code.replace(/\n$/, ""));
      button.textContent = "Copied";
    } catch {
      button.textContent = "Couldn't copy";
    }
    window.setTimeout(() => {
      button.textContent = COPY_LABEL;
    }, COPY_FEEDBACK_MS);
  }

  function onclick(event: MouseEvent): void {
    if (!(event.target instanceof Element)) return;
    const copy = event.target.closest<HTMLButtonElement>("button[data-copy-code]");
    if (copy !== null) {
      void copyCode(copy);
      return;
    }
    const link = event.target.closest<HTMLAnchorElement>("a[data-path]");
    const newTab = event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey;
    if (link === null || newTab || event.defaultPrevented) return;
    event.preventDefault();
    openPathInPanel(agent, link.dataset.path ?? "");
  }

  // The links and buttons are in the rendered HTML, so their clicks are
  // handled here; keyboard activation arrives as a click too.
  function handleClicks(node: HTMLElement): () => void {
    node.addEventListener("click", onclick);
    return () => node.removeEventListener("click", onclick);
  }
</script>

<div class="prose" data-size={size} {@attach render} {@attach handleClicks}></div>

<style>
  .prose {
    min-width: 0;
    color: var(--color-text);
    font-size: var(--font-size-message);
    line-height: var(--line-height-message);
    overflow-wrap: anywhere;

    &[data-size="compact"] {
      font-size: var(--font-size-ui);
    }
  }

  /* The rendered nodes carry no scoping class, so everything under the root is global. */
  .prose :global {
    :is(p, ul, ol, blockquote, .prose-code, table, hr) {
      margin-bottom: var(--space-10);
    }

    > :last-child {
      margin-bottom: 0;
    }

    :is(h1, h2, h3, h4, h5, h6) {
      margin: var(--space-14) 0 var(--space-6);
      font-size: var(--font-size-message);
      font-weight: var(--font-weight-semibold);
      line-height: var(--line-height-tight);
    }

    :is(h1, h2) {
      font-size: var(--font-size-heading);
    }

    > :first-child {
      margin-top: 0;
    }

    :is(ul, ol) {
      padding-left: var(--space-24);
    }

    li {
      margin: var(--space-2) 0;
    }

    li::marker {
      color: var(--color-text-3);
    }

    :is(strong, b) {
      font-weight: var(--font-weight-semibold);
    }

    code {
      padding: 1px var(--space-4);
      border-radius: var(--corner-sm);
      background: var(--color-stone-3);
      font-family: var(--font-code);
      font-size: var(--font-size-code);
    }

    a {
      color: var(--color-vein-bright);
      text-decoration-line: underline;
      text-decoration-color: var(--color-vein-line);
      text-underline-offset: 2px;
      transition: text-decoration-color var(--duration-fast) var(--ease-out);
    }

    a:hover {
      text-decoration-color: currentcolor;
    }

    /* A path link underlines its code, not the gap around it. */
    .prose-path {
      text-decoration: none;
    }

    .prose-path code {
      color: var(--color-vein-bright);
      text-decoration-line: underline;
      text-decoration-color: var(--color-vein-line);
      text-underline-offset: 2px;
    }

    .prose-path:hover code {
      text-decoration-color: currentcolor;
    }

    blockquote {
      padding-left: var(--space-12);
      border-left: 2px solid var(--color-line);
      color: var(--color-text-2);
    }

    hr {
      height: 1px;
      border: 0;
      background: var(--color-line-soft);
    }

    img {
      max-width: 100%;
      border-radius: var(--corner-md);
    }

    table {
      display: block;
      max-width: 100%;
      overflow-x: auto;
      border-collapse: collapse;
      font-size: var(--font-size-sm);
    }

    :is(th, td) {
      padding: var(--space-6) var(--space-12);
      border-bottom: 1px solid var(--color-line-soft);
      text-align: left;
    }

    th {
      color: var(--color-text-2);
      font-weight: var(--font-weight-medium);
    }

    /* A code block: a well with its Copy button in the corner. */
    .prose-code {
      position: relative;
    }

    pre {
      padding: var(--space-12) var(--space-64) var(--space-12) var(--space-14);
      overflow-x: auto;
      border-radius: var(--corner-md);
      background: var(--color-stone-2);
      font-family: var(--font-code);
      font-size: var(--font-size-xs);
      line-height: var(--line-height-ui);
      overflow-wrap: normal;
    }

    pre code {
      padding: 0;
      background: none;
      font-size: inherit;
    }

    .prose-copy {
      position: absolute;
      top: var(--space-6);
      right: var(--space-6);
      min-height: 26px;
      padding: 0 var(--space-8);
      border: 0;
      border-radius: var(--corner-sm);
      background: none;
      color: var(--color-text-2);
      cursor: pointer;
      font-family: var(--font-ui);
      font-size: var(--font-size-xs);
      transition:
        background var(--duration-fast) var(--ease-out),
        color var(--duration-fast) var(--ease-out);
    }

    .prose-copy:hover {
      background: var(--color-stone-3);
      color: var(--color-text);
    }
  }

  @media (max-width: 760px) {
    .prose :global(.prose-copy) {
      top: 0;
      right: 0;
      min-height: var(--layout-touch-target);
    }
  }
</style>
