import { createRawSnippet, type Snippet } from "svelte";

export function htmlSnippet(html: string): Snippet {
  return createRawSnippet(() => ({ render: () => html }));
}
