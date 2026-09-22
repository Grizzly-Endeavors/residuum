<script lang="ts">
  import { tick } from "svelte";
  import { FeedScroller } from "../lib/feed-scroll.svelte";
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
  let innerEl: HTMLDivElement | undefined = $state();
  let lastTailId: number | undefined;

  // Follow the tail as frames stream in while the reader is at the bottom;
  // once they scroll up to read, leave them there and offer the way back.
  const scroller = new FeedScroller();

  $effect(() => {
    if (!feedEl || !innerEl) return;
    return scroller.attach(feedEl, innerEl);
  });

  $effect(() => {
    const tail = items[items.length - 1];
    const tailChanged = tail?.id !== lastTailId;
    lastTailId = tail?.id;
    void working;
    // Opening the transcript, or the owner's own message, always scrolls down.
    const force = (tailChanged && tail?.kind === "user") || loading;
    void tick().then(() => {
      scroller.contentChanged(force);
    });
  });
</script>

<!-- The pill sits outside the scroller so it stays put at the top of the feed. -->
<div class="session-feed-frame">
  {#if scroller.scrolledUp && !loading}
    <div class="anchor-pill">
      <button type="button" class="anchor-pill-jump" onclick={() => scroller.jumpToLatest()}>
        Jump to latest
      </button>
    </div>
  {/if}
  <div class="chat-feed session-feed" bind:this={feedEl}>
    <div class="chat-feed-inner" aria-busy={loading} bind:this={innerEl}>
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
</div>
