<script lang="ts">
  import { tick } from "svelte";
  import type { FeedItem } from "../lib/types";
  import MessageUser from "./MessageUser.svelte";
  import MessageAssistant from "./MessageAssistant.svelte";
  import MessageSystem from "./MessageSystem.svelte";
  import MessageError from "./MessageError.svelte";
  import MessageNotice from "./MessageNotice.svelte";
  import MessageDivider from "./MessageDivider.svelte";
  import ToolGroup from "./ToolGroup.svelte";
  import ThinkingIndicator from "./ThinkingIndicator.svelte";

  let {
    items,
    isProcessing,
    verbose,
  }: {
    items: FeedItem[];
    isProcessing: boolean;
    verbose: boolean;
  } = $props();

  let feedEl: HTMLDivElement | undefined = $state();

  function scrollToBottom() {
    if (!feedEl) return;
    window.requestAnimationFrame(() => {
      if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
    });
  }

  $effect(() => {
    // scroll when items change
    items.length;
    isProcessing;
    void tick().then(scrollToBottom);
  });
</script>

<div class="chat-feed" bind:this={feedEl}>
  <div class="chat-feed-inner">
    {#each items as item (item.id)}
      {#if item.kind === "user"}
        <MessageUser content={item.content} />
      {:else if item.kind === "assistant"}
        <MessageAssistant content={item.content} />
      {:else if item.kind === "system"}
        <MessageSystem content={item.content} />
      {:else if item.kind === "error"}
        <MessageError content={item.content} />
      {:else if item.kind === "notice"}
        <MessageNotice content={item.content} />
      {:else if item.kind === "divider"}
        <MessageDivider label={item.label} />
      {:else if item.kind === "tool-group"}
        <ToolGroup calls={item.calls} {verbose} />
      {:else if item.kind === "command-output"}
        <MessageNotice content={item.content} />
      {/if}
    {/each}
    {#if isProcessing}
      <ThinkingIndicator />
    {/if}
  </div>
</div>
