<script lang="ts">
  import ToolGroup from "../components/ToolGroup.svelte";
  import type { FeedItem } from "../lib/types";
  import AgentMessageCard from "./AgentMessageCard.svelte";
  import CompressedMarker from "./CompressedMarker.svelte";
  import FeedDivider from "./FeedDivider.svelte";
  import FileAttachment from "./FileAttachment.svelte";
  import LocalNote from "./LocalNote.svelte";
  import Prose from "./Prose.svelte";
  import StatusLine from "./StatusLine.svelte";
  import UserMessage from "./UserMessage.svelte";

  // One feed item, in the main chat or a session's transcript. `agent` is the
  // agent the conversation belongs to, which its links and Undo act on.
  // Tool calls show the legacy tool rows, and only while "Show tool calls" is on.

  let { item, agent }: { item: FeedItem; agent: string } = $props();
</script>

{#if item.kind === "user"}
  <UserMessage {item} {agent} />
{:else if item.kind === "assistant"}
  <Prose content={item.content} {agent} />
{:else if item.kind === "agent-message"}
  <AgentMessageCard {item} {agent} />
{:else if item.kind === "divider"}
  <FeedDivider label={item.label} />
{:else if item.kind === "compressed-marker"}
  <CompressedMarker {agent} />
{:else if item.kind === "file-attachment"}
  <FileAttachment {item} />
{:else if item.kind === "status"}
  <StatusLine tone={item.tone} content={item.content} details={item.details} />
{:else if item.kind === "local-system"}
  <LocalNote content={item.content} />
{:else}
  <div data-legacy-view>
    <ToolGroup calls={item.calls} verbose />
  </div>
{/if}
