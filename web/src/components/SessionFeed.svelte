<script lang="ts">
  import { tick } from "svelte";
  import type { FeedItem } from "../lib/types";
  import FeedItemView from "./FeedItemView.svelte";
  import ThinkingIndicator from "./ThinkingIndicator.svelte";

  let {
    items,
    verbose,
    working,
    loading,
    loadError,
    onRetry,
  }: {
    items: FeedItem[];
    verbose: boolean;
    working: boolean;
    loading: boolean;
    loadError: string | null;
    onRetry: () => void;
  } = $props();

  let feedEl: HTMLDivElement | undefined = $state();
  let lastTailId: number | undefined;

  // Follow the tail as frames stream in, like the main chat.
  $effect(() => {
    const tailId = items[items.length - 1]?.id;
    void working;
    if (tailId === lastTailId && !working) return;
    lastTailId = tailId;
    void tick().then(() => {
      if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
    });
  });
</script>

<div class="chat-feed session-feed" bind:this={feedEl}>
  <div class="chat-feed-inner" aria-busy={loading}>
    {#if loading}
      <p class="chat-feed-empty">Loading transcript…</p>
    {:else if loadError}
      <div class="session-feed-error" role="alert">
        <p>{loadError}</p>
        <button type="button" class="sessions-text-btn" onclick={onRetry}>Try again</button>
      </div>
    {:else}
      {#each items as item (item.id)}
        <FeedItemView {item} {verbose} />
      {:else}
        <p class="chat-feed-empty">No messages in this run yet.</p>
      {/each}
      {#if working}
        <ThinkingIndicator />
      {/if}
    {/if}
  </div>
</div>
