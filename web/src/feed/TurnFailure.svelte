<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { Icon } from "../lib/icons";
  import type { TurnFailureFeedItem } from "../lib/types";
  import { Button, Disclosure } from "../lib/ui";
  import { ws } from "../lib/ws.svelte";

  // The end of a turn that couldn't finish: what went wrong in plain words,
  // the technical detail behind Details, and Try again, which sends what the
  // user wrote once more. It stays after the toast for the failure is gone.

  let { item, agent }: { item: TurnFailureFeedItem; agent: string } = $props();

  /** The user sent it again from here, so the message below it is the answer to the button. */
  let sentAgain = $state(false);

  const retry = $derived(ws.agent === agent ? item.retry : undefined);

  function tryAgain(): void {
    if (retry === undefined) return;
    sentAgain = true;
    ws.sendChat(retry.content, retry.images);
  }
</script>

<div class="turn-failure">
  <span class="failure-icon"><Icon name="warning" size={16} /></span>
  <div class="failure-body">
    <p class="failure-title">{hub.shownName(agent)} couldn't finish this reply</p>
    <p class="failure-message">{item.message}</p>
    {#if retry !== undefined && !sentAgain}
      <div class="failure-actions">
        <Button variant="secondary" size="sm" icon="reload" onclick={tryAgain}>Try again</Button>
      </div>
    {/if}
    {#if item.details}
      <Disclosure summary="Details" tone="quiet">
        <pre class="failure-details">{item.details}</pre>
      </Disclosure>
    {/if}
  </div>
</div>

<style>
  .turn-failure {
    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
    padding: var(--space-10) var(--space-14);
    border-radius: var(--corner-md);
    background: var(--color-err-tint);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .failure-icon {
    display: grid;
    flex: none;
    place-items: center;
    width: 16px;
    height: calc(var(--font-size-ui) * var(--line-height-ui));
    color: var(--color-err-text);
  }

  .failure-body {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-4);
    min-width: 0;
  }

  .failure-title {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-medium);
    line-height: var(--line-height-ui);
  }

  .failure-message {
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  .failure-actions {
    margin-top: var(--space-4);
  }

  .failure-body :global(.ui-disclosure) {
    align-self: stretch;
  }

  .failure-details {
    margin-top: var(--space-4);
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
