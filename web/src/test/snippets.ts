import { createRawSnippet, type Snippet } from "svelte";

/**
 * A snippet that renders `html`, for passing children to a component under
 * test. The markup must have a single root element.
 */
export function htmlSnippet(html: string): Snippet {
  return createRawSnippet(() => ({ render: () => html }));
}
