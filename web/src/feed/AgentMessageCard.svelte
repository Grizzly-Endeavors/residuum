<script lang="ts">
  import { Icon } from "../lib/icons";
  import { openSessionByAddress } from "../lib/session-address";
  import type { AgentMessageFeedItem } from "../lib/types";
  import { Button } from "../lib/ui";
  import { ws } from "../lib/ws.svelte";
  import { cardSender } from "./feed-words";
  import Prose from "./Prose.svelte";
  import Timestamp from "./Timestamp.svelte";

  // A message from a session or a teammate: a compact card with who sent it,
  // when, and its body clamped to a few lines. A session sender, on `agent`, can be
  // opened; a teammate is another agent and has no session here to open.

  let { item, agent }: { item: AgentMessageFeedItem; agent: string } = $props();

  const uid = $props.id();
  const category = $derived(
    item.category ?? (ws.agent === agent ? ws.sessions.findByAddress(item.from)?.category : null),
  );
  const sender = $derived(cardSender(item.from, category ?? null, agent));

  let expanded = $state(false);
  let overflowing = $state(false);

  // The body is clamped until Show all; whether it overflows changes as the
  // text renders, fonts load and the column resizes.
  function measure(node: HTMLElement): () => void {
    const check = (): void => {
      overflowing = node.scrollHeight > node.clientHeight + 1;
    };
    const observer = new ResizeObserver(check);
    observer.observe(node);
    if (node.firstElementChild) observer.observe(node.firstElementChild);
    check();
    return () => observer.disconnect();
  }
</script>

<article class="agent-card" aria-label="{sender.kind}: {sender.sender}">
  <header class="card-head">
    <Icon name={sender.icon} size={14} />
    <span class="card-kind">{sender.kind}</span>
    <span class="card-sender">{sender.sender}</span>
    {#if item.timestamp}
      <span class="card-time"><Timestamp timestamp={item.timestamp} /></span>
    {/if}
  </header>
  <div
    class="card-body"
    class:clamped={!expanded}
    class:faded={!expanded && overflowing}
    id="{uid}-body"
    {@attach measure}
  >
    <Prose content={item.content} {agent} size="compact" />
  </div>
  {#if overflowing || expanded || sender.isSession}
    <div class="card-actions">
      {#if overflowing || expanded}
        <Button
          variant="quiet"
          size="sm"
          aria-expanded={expanded}
          aria-controls="{uid}-body"
          onclick={() => (expanded = !expanded)}
        >
          {expanded ? "Show less" : "Show all"}
        </Button>
      {/if}
      {#if sender.isSession}
        <Button
          variant="quiet"
          size="sm"
          icon="sessions"
          onclick={() => void openSessionByAddress(agent, item.from, item.runId)}
        >
          Open session
        </Button>
      {/if}
    </div>
  {/if}
</article>

<style>
  .agent-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12) var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-width: 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    & :global(svg) {
      flex: none;
      color: var(--color-vein-bright);
    }
  }

  .card-kind {
    color: var(--color-text-2);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
  }

  .card-sender {
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-code);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* At the end of the line, once its sender has said what it needs. */
  .card-time {
    flex: none;
    margin-left: auto;
  }

  .card-body.clamped {
    max-height: calc(var(--line-height-message) * 8em);
    overflow: hidden;
  }

  .card-body.faded {
    mask-image: linear-gradient(to bottom, var(--color-text) 70%, transparent);
  }

  .card-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4);
    margin: 0 0 calc(-1 * var(--space-4)) calc(-1 * var(--space-8));
  }
</style>
