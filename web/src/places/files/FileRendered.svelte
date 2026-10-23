<script lang="ts">
  import { openAgentFileInPanel } from "../../feed/feed-links";
  import Prose from "../../feed/Prose.svelte";
  import { splitFrontmatter } from "./frontmatter";
  import type { FileSource } from "./file-source";

  // A file's content as read, not edited: Markdown through `feed/Prose.svelte`
  // (the same sanitizer and image rules as a reply), its leading frontmatter
  // as a key/value block, or plain text with its characters shown as written.
  // Relative links to other Markdown files open them in the same tree. In a
  // panel that names its agent, a link opens as that agent's file in the same
  // way, so the place under the panel (the Inbox) stays.

  interface Props {
    /** The file's text. */
    content: string;
    /** The file's path in its tree (the team's folder, for a team source). */
    path: string;
    /** The tree the file is in: an agent's workspace, or the team's folder. */
    source: FileSource;
    /** `markdown` renders the text; `text` shows it as it is, wrapped and unformatted. */
    mode: "markdown" | "text";
    /** The panel names the file's agent, so a link opens as that agent's file whatever place the panel is beside. */
    agentNamed?: boolean;
  }

  let { content, path, source, mode, agentNamed = false }: Props = $props();

  const split = $derived(mode === "markdown" ? splitFrontmatter(content) : null);
  const openAgentPath = $derived.by(() => {
    const { agent } = source;
    return agentNamed && agent !== null
      ? (linked: string): void => openAgentFileInPanel(agent, linked)
      : undefined;
  });
</script>

{#if mode === "text"}
  <pre class="file-rendered-text">{content}</pre>
{:else}
  <div class="file-rendered">
    {#if split !== null}
      <dl class="file-rendered-frontmatter">
        {#each split.fields as field, index (index)}
          <div class="file-rendered-field">
            <dt>{field.key}</dt>
            <dd>{field.value}</dd>
          </div>
        {/each}
      </dl>
    {/if}
    <Prose
      content={split === null ? content : split.body}
      agent={source.agent}
      teamFolder={source.scope === "team"}
      filePath={path}
      openPath={openAgentPath}
      size="compact"
    />
  </div>
{/if}

<style>
  .file-rendered-text {
    margin: 0;
    color: var(--color-text);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    --leading: var(--line-height-message);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .file-rendered-frontmatter {
    display: grid;
    margin: 0 0 var(--space-14);
    padding: var(--space-8) var(--space-12);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-sm);
    background: var(--color-stone-1);
    font-size: var(--font-size-sm);
  }

  .file-rendered-field {
    display: grid;
    grid-template-columns: minmax(6rem, max-content) minmax(0, 1fr);
    gap: var(--space-12);
    padding: var(--space-4) 0;
  }

  .file-rendered-field dt {
    color: var(--color-text-3);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
  }

  .file-rendered-field dd {
    margin: 0;
    min-width: 0;
    color: var(--color-text);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
