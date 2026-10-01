<script lang="ts">
  import { diffLineKind } from "../../lib/checkpoints";

  // A file at a checkpoint: what changed, its lines colored as added or
  // removed, or its whole text. A file's history and Settings → History show
  // it. It scrolls, so it takes focus for the keyboard.

  interface Props {
    text: string;
    as: "diff" | "file";
    /** Names the region: "Changes to SOUL.md". */
    label: string;
  }

  let { text, as, label }: Props = $props();
</script>

<!-- A scroll box takes focus so the keyboard can scroll it. -->
{#if as === "file"}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <pre class="checkpoint-text" role="region" tabindex="0" aria-label={label}>{text}</pre>
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="checkpoint-text" role="region" tabindex="0" aria-label={label}>
    {#each text.replace(/\n$/, "").split("\n") as line, index (index)}
      <span class="checkpoint-line" data-kind={diffLineKind(line)}>{line}</span>
    {/each}
  </div>
{/if}

<style>
  .checkpoint-text {
    max-height: 40vh;
    margin: 0;
    padding: var(--space-10) var(--space-12);
    overflow: auto;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  div.checkpoint-text {
    white-space: normal;
  }

  .checkpoint-line {
    display: block;
    min-height: calc(var(--line-height-ui) * 1em);
    white-space: pre-wrap;

    &[data-kind="added"] {
      color: var(--color-moss-text);
    }

    &[data-kind="removed"] {
      color: var(--color-err-text);
    }

    &[data-kind="meta"] {
      color: var(--color-text-3);
    }
  }

  @media (max-width: 760px) {
    .checkpoint-text {
      max-height: none;
    }
  }
</style>
