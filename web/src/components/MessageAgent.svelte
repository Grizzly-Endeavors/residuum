<script lang="ts">
  import MarkdownContent from "./MarkdownContent.svelte";
  import { ws } from "../lib/ws.svelte";
  import type { AgentMessageFeedItem } from "../lib/types";
  import CategoryBadge from "./CategoryBadge.svelte";

  let { item }: { item: AgentMessageFeedItem } = $props();

  let expanded = $state(false);
  let overflowing = $state(false);
  let body: HTMLDivElement | undefined = $state();
  const bodyId = `agent-msg-${Math.random().toString(36).slice(2, 10)}`;

  let category = $derived(item.category ?? ws.sessions.findByAddress(item.from)?.category ?? null);
  let isSession = $derived(item.from !== "main");

  $effect(() => {
    void item.content;
    if (!body || expanded) return;
    overflowing = body.scrollHeight > body.clientHeight + 1;
  });

  function open() {
    void ws.sessions.openAddress(item.from, item.runId);
  }
</script>

<div class="msg msg-agent" role="group" aria-label="Message from {item.from}">
  <div class="msg-agent-head">
    {#if category && isSession}
      <CategoryBadge {category} />
    {/if}
    <span class="msg-agent-from">
      {isSession ? item.from : "main agent"}
    </span>
    {#if isSession}
      <button type="button" class="msg-agent-open" onclick={open}>Open session</button>
    {/if}
  </div>
  <div class="msg-content msg-agent-body" class:clamped={!expanded} id={bodyId} bind:this={body}>
    <MarkdownContent content={item.content} />
  </div>
  {#if overflowing || expanded}
    <button
      type="button"
      class="msg-agent-toggle"
      aria-expanded={expanded}
      aria-controls={bodyId}
      onclick={() => (expanded = !expanded)}
    >
      {expanded ? "Show less" : "Show all"}
    </button>
  {/if}
</div>
