<script lang="ts">
  import type { ToolCallState } from "../lib/types";
  import {
    formatToolResult,
    toolResultToggleLabel,
    type ToolResultCollapse,
  } from "../lib/format-tool-result";
  import ToolArgs from "./ToolArgs.svelte";

  let { call }: { call: ToolCallState } = $props();
  let open = $state(false);
  /** Result text the inner "show all" was opened for. A new result starts collapsed. */
  let expandedResult = $state<string | null>(null);

  const formatted = $derived(
    call.result ? formatToolResult(call.result, { isError: call.status === "error" }) : null,
  );
  const expanded = $derived(expandedResult === call.result);

  function shownSlice<T>(
    items: readonly T[],
    collapse: ToolResultCollapse,
    isExpanded: boolean,
  ): readonly T[] {
    if (!collapse.long || isExpanded || collapse.hiddenLines === 0) return items;
    return items.slice(0, items.length - collapse.hiddenLines);
  }

  function toggleExpanded(): void {
    expandedResult = expanded ? null : (call.result ?? null);
  }
</script>

<div class="tool-item" class:open class:err={call.status === "error" || formatted?.error === true}>
  <div
    class="tool-header"
    onclick={() => (open = !open)}
    role="button"
    tabindex="0"
    aria-expanded={open}
    onkeydown={(e) => {
      if (e.key === "Enter" || e.key === " ") open = !open;
    }}
  >
    <span class="tool-chevron">&#9654;</span>
    <span class="tool-name">{call.name}</span>
    <ToolArgs name={call.name} args={call.arguments} />
    <span class="tool-status" class:ok={call.status === "done"} class:err={call.status === "error"}>
      {call.status === "running" ? "running..." : call.status}
    </span>
  </div>
  {#if formatted}
    <div
      class="tool-body"
      class:err={formatted.error}
      class:json={formatted.shape === "json"}
      class:file={formatted.shape === "file"}
      class:list={formatted.shape === "list"}
    >
      {#if formatted.label}
        <div class="tool-result-label">{formatted.label}</div>
      {/if}
      <div
        class="tool-result-body"
        class:clamped={formatted.collapse.long && !expanded && formatted.collapse.hiddenLines === 0}
        class:expanded={formatted.collapse.long && expanded}
      >
        {#if formatted.shape === "file"}
          <div class="tool-file">
            {#each shownSlice(formatted.rows, formatted.collapse, expanded) as row, index (index)}
              {#if row.kind === "gap"}
                <div class="tool-file-gap">{row.text}</div>
              {:else}
                <div class="tool-file-line">
                  <span class="tool-file-gutter">{row.number}</span>
                  <span class="tool-file-code">{row.text}</span>
                </div>
              {/if}
            {/each}
          </div>
        {:else if formatted.shape === "list"}
          <ul class="tool-result-list">
            {#each shownSlice(formatted.items, formatted.collapse, expanded) as item, index (index)}
              <li>{item}</li>
            {/each}
          </ul>
        {:else}
          {expanded || !formatted.collapse.long ? formatted.text : formatted.preview}
        {/if}
      </div>
      {#if formatted.collapse.long}
        <button
          type="button"
          class="tool-result-toggle"
          aria-expanded={expanded}
          onclick={toggleExpanded}
        >
          {toolResultToggleLabel(formatted.collapse.hiddenLines, expanded)}
        </button>
      {/if}
    </div>
  {/if}
</div>
