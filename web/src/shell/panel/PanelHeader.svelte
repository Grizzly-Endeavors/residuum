<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon, type IconName } from "../../lib/icons";
  import { IconButton } from "../../lib/ui";
  import { panelFrame } from "./panel-frame";

  // The top of whatever the context panel shows: what it is, its title (which
  // names the panel), a line of details, its own actions, and the way out:
  // Close beside the main region, Back on a phone's full-screen sheet.

  interface Props {
    icon: IconName;
    title: string;
    /** A small line above the title saying what kind of thing this is, such as "File". */
    kind?: string;
    /** Set the title in the code face, for a file name or an id. */
    code?: boolean;
    /** Details under the title. */
    meta?: Snippet;
    /** Buttons beside the way out, for what the panel shows. */
    actions?: Snippet;
  }

  let { icon, title, kind, code = false, meta, actions }: Props = $props();

  const frame = panelFrame();
</script>

<header class="context-panel-head">
  {#if frame.layout === "phone"}
    <IconButton icon="chevron-left" label="Back" onclick={frame.close} />
  {/if}
  <div class="context-panel-titles">
    {#if kind !== undefined}
      <p class="context-panel-kind"><Icon name={icon} size={13} />{kind}</p>
    {/if}
    <h2 id={frame.titleId} class="context-panel-title" class:is-code={code}>
      {#if kind === undefined}<Icon name={icon} size={15} />{/if}
      <span class="context-panel-title-text">{title}</span>
    </h2>
    {#if meta}
      <div class="context-panel-meta">{@render meta()}</div>
    {/if}
  </div>
  {#if actions}
    <div class="context-panel-actions">{@render actions()}</div>
  {/if}
  {#if frame.layout !== "phone"}
    <IconButton icon="close" label="Close panel" size="sm" onclick={frame.close} />
  {/if}
</header>

<style>
  /* At least as tall as a place's header, so the two hairlines meet across the shell. */
  .context-panel-head {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-8);
    min-height: var(--layout-place-header-height);
    padding: var(--space-8) var(--space-10) var(--space-8) var(--space-18);
    border-bottom: 1px solid var(--color-line-soft);
  }

  .context-panel-titles {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .context-panel-kind {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    & :global(svg) {
      color: var(--color-vein-bright);
    }
  }

  .context-panel-title {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-width: 0;
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);

    & :global(svg) {
      flex: none;
      color: var(--color-text-2);
    }

    &.is-code {
      font-family: var(--font-code);
      font-size: var(--font-size-ui);
      font-weight: var(--font-weight-medium);
    }
  }

  .context-panel-title-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .context-panel-meta {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4) var(--space-12);
    min-width: 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }

  .context-panel-actions {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-4);
  }

  @media (max-width: 760px) {
    .context-panel-head {
      padding: var(--space-6) var(--space-12) var(--space-6) var(--space-4);
    }
  }
</style>
