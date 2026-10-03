<script lang="ts">
  import {
    formatToolResult,
    toolResultToggleLabel,
    type ToolResultCollapse,
  } from "../lib/format-tool-result";
  import type { StepCall } from "./activity";
  import { autoModeNote } from "./auto-mode";
  import { argumentLines } from "./step-args";

  // A step's details: its arguments, summarized per tool, and what it
  // returned, shaped by kind: JSON indented, a file with its line numbers, a
  // list, or text. A long result or quoted text starts clamped, with Show all.

  let { call }: { call: StepCall } = $props();

  /** Quoted text longer than this starts clamped. */
  const QUOTE_CLAMP_CHARS = 240;

  const lines = $derived(argumentLines(call.name, call.arguments));
  const result = $derived(
    call.result ? formatToolResult(call.result, { isError: call.status === "error" }) : null,
  );
  /** The result text Show all was pressed for; a new result starts clamped again. */
  let expandedFor = $state<string | null>(null);
  const expanded = $derived(expandedFor !== null && expandedFor === call.result);
  let quoteOpen = $state(false);

  function shownSlice<T>(items: readonly T[], collapse: ToolResultCollapse): readonly T[] {
    if (!collapse.long || expanded || collapse.hiddenLines === 0) return items;
    return items.slice(0, items.length - collapse.hiddenLines);
  }
</script>

<div class="step-detail">
  {#if call.autoMode}
    <p class="step-auto-mode" data-decision={call.autoMode.decision}>
      {autoModeNote(call.autoMode)}
    </p>
  {/if}
  {#each lines as line, index (index)}
    {#if line.kind === "pairs"}
      <dl class="step-pairs">
        {#each line.pairs as [name, value] (name)}
          <dt>{name}</dt>
          <dd>{value}</dd>
        {/each}
      </dl>
    {:else if line.kind === "quote"}
      {@const long = line.text.length > QUOTE_CLAMP_CHARS}
      <p class="step-quote" class:clamped={long && !quoteOpen}>“{line.text}”</p>
      {#if long}
        <button
          type="button"
          class="step-more"
          aria-expanded={quoteOpen}
          onclick={() => (quoteOpen = !quoteOpen)}>{quoteOpen ? "Show less" : "Show all"}</button
        >
      {/if}
    {:else}
      <p class="step-arg" data-kind={line.kind}>
        {line.kind === "query" ? `“${line.text}”` : line.text}
      </p>
    {/if}
  {/each}

  {#if result}
    <div class="step-result" class:failed={result.error}>
      {#if result.label}
        <p class="step-result-label">{result.label}</p>
      {/if}
      <div
        class="step-result-body"
        class:clamped={result.collapse.long && !expanded && result.collapse.hiddenLines === 0}
      >
        {#if result.shape === "file"}
          {#each shownSlice(result.rows, result.collapse) as row, index (index)}
            {#if row.kind === "gap"}
              <div class="step-file-gap">{row.text}</div>
            {:else}
              <div class="step-file-line">
                <span class="step-file-number" aria-hidden="true">{row.number}</span>
                <span>{row.text}</span>
              </div>
            {/if}
          {/each}
        {:else if result.shape === "list"}
          <ul class="step-list">
            {#each shownSlice(result.items, result.collapse) as item, index (index)}
              <li>{item}</li>
            {/each}
          </ul>
        {:else}
          {expanded || !result.collapse.long ? result.text : result.preview}
        {/if}
      </div>
      {#if result.collapse.long}
        <button
          type="button"
          class="step-more"
          aria-expanded={expanded}
          onclick={() => (expandedFor = expanded ? null : (call.result ?? null))}
        >
          {toolResultToggleLabel(result.collapse.hiddenLines, expanded)}
        </button>
      {/if}
    </div>
  {:else if call.status === "running"}
    <p class="step-pending">Waiting for it to finish.</p>
  {:else if call.status === "stopped"}
    <p class="step-pending">Stopped before it returned anything.</p>
  {/if}
</div>

<style>
  .step-detail {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    min-width: 0;
    padding: var(--space-8) var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
  }

  .step-arg {
    overflow-wrap: anywhere;

    &[data-kind="code"],
    &[data-kind="path"] {
      color: var(--color-text);
      font-family: var(--font-code);
      white-space: pre-wrap;
    }

    &[data-kind="query"],
    &[data-kind="label"] {
      color: var(--color-text);
    }
  }

  .step-quote {
    white-space: pre-wrap;
    overflow-wrap: anywhere;

    &.clamped {
      display: -webkit-box;
      overflow: hidden;
      -webkit-box-orient: vertical;
      -webkit-line-clamp: 4;
      line-clamp: 4;
    }
  }

  .step-pairs {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--space-2) var(--space-12);

    & dt {
      color: var(--color-text-3);
      font-family: var(--font-code);
    }

    & dd {
      font-family: var(--font-code);
      white-space: pre-wrap;
      overflow-wrap: anywhere;
    }
  }

  .step-result {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
    padding-top: var(--space-6);
    border-top: 1px solid var(--color-line-soft);

    &.failed .step-result-body {
      color: var(--color-err-text);
    }
  }

  .step-result-label {
    color: var(--color-text-3);
  }

  .step-result-body {
    overflow-x: auto;
    font-family: var(--font-code);
    white-space: pre-wrap;
    overflow-wrap: anywhere;

    &.clamped {
      max-height: 16lh;
      overflow-y: hidden;
    }
  }

  .step-file-line {
    display: grid;
    grid-template-columns: 4ch minmax(0, 1fr);
    gap: var(--space-10);
  }

  .step-file-number {
    color: var(--color-text-3);
    text-align: end;
    user-select: none;
  }

  .step-file-gap {
    color: var(--color-text-3);
  }

  .step-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding-left: var(--space-16);
    list-style: disc;
  }

  .step-more {
    align-self: flex-start;
    min-height: 24px;
    color: var(--color-vein-bright);
    font-family: var(--font-ui);
    font-size: var(--font-size-xs);

    &:hover {
      text-decoration: underline;
    }
  }

  .step-pending {
    color: var(--color-text-3);
  }

  .step-auto-mode {
    color: var(--color-text-3);

    &[data-decision="blocked"] {
      color: var(--color-err-text);
    }
  }

  @media (max-width: 760px) {
    .step-more {
      min-height: var(--layout-touch-target);
    }
  }
</style>
