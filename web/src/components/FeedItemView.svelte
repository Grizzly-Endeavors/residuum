<script lang="ts">
  import type { FeedItem } from "../lib/types";
  import MessageUser from "./MessageUser.svelte";
  import MessageAssistant from "./MessageAssistant.svelte";
  import MessageDivider from "./MessageDivider.svelte";
  import MessageLocalSystem from "./MessageLocalSystem.svelte";
  import MessageAgent from "./MessageAgent.svelte";
  import MessageStatus from "./MessageStatus.svelte";
  import CompressedHistoryMarker from "./CompressedHistoryMarker.svelte";
  import ToolGroup from "./ToolGroup.svelte";
  import FileAttachment from "./FileAttachment.svelte";

  let { item, verbose }: { item: FeedItem; verbose: boolean } = $props();
</script>

{#if item.kind === "user"}
  <MessageUser content={item.content} images={item.images} sender={item.sender} />
{:else if item.kind === "assistant"}
  <MessageAssistant content={item.content} />
{:else if item.kind === "divider"}
  <MessageDivider label={item.label} variant={item.variant ?? "day"} />
{:else if item.kind === "compressed-marker"}
  <CompressedHistoryMarker />
{:else if item.kind === "tool-group"}
  <ToolGroup calls={item.calls} {verbose} />
{:else if item.kind === "file-attachment"}
  <FileAttachment {item} />
{:else if item.kind === "local-system"}
  <MessageLocalSystem content={item.content} />
{:else if item.kind === "agent-message"}
  <MessageAgent {item} />
{:else if item.kind === "status"}
  <MessageStatus tone={item.tone} content={item.content} />
{/if}
