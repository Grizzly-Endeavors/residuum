<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { DeletedAgent } from "../../lib/hub-types";
  import { hub } from "../../lib/hub.svelte";
  import { relativeTime } from "../../lib/time";
  import { Button, Disclosure } from "../../lib/ui";
  import { agentLifecycle } from "./agent-lifecycle.svelte";

  // Recently deleted, collapsed under the board: every deleted agent that can
  // be restored, with Restore. It shows once there is something to list or a
  // failed load to try again, and stays while it is open.

  let { now }: { now: number } = $props();

  let open = $state(false);
  let container = $state<HTMLElement>();

  onMount(() => {
    void hub.refreshDeleted();
  });

  async function restore(gone: DeletedAgent): Promise<void> {
    await agentLifecycle.restore(gone.name, gone.checkpoint_id);
    await tick();
    // A restored agent's row goes with its button, so focus goes back to the disclosure.
    if (container !== undefined && !container.contains(document.activeElement)) {
      container.querySelector<HTMLElement>("[aria-expanded]")?.focus();
    }
  }
</script>

{#if open || hub.deletedError !== null || hub.deleted.length > 0}
  <div class="deleted" bind:this={container}>
    <Disclosure summary="Recently deleted" bind:open>
      {#if hub.deletedError !== null}
        <p class="deleted-problem" role="alert">
          {hub.deletedError}
          <Button variant="quiet" size="sm" onclick={() => void hub.refreshDeleted()}
            >Try again</Button
          >
        </p>
      {:else if hub.deleted.length === 0}
        <p class="deleted-hint">Nothing to restore.</p>
      {:else}
        <p class="deleted-hint">
          A deleted agent's files stay in its checkpoint history. Restoring brings back its notes,
          memory, settings and role page.
        </p>
        <ul class="deleted-list">
          {#each hub.deleted as gone (gone.name)}
            <li class="deleted-row">
              <span class="deleted-name">{gone.name}</span>
              <span class="deleted-when">Deleted {relativeTime(gone.deleted_at, now)}</span>
              <Button
                size="sm"
                icon="restore"
                loading={agentLifecycle.pendingOf(gone.name) === "restore"}
                aria-label="Restore {gone.name}"
                onclick={() => void restore(gone)}>Restore</Button
              >
            </li>
          {/each}
        </ul>
      {/if}
    </Disclosure>
  </div>
{/if}

<style>
  .deleted {
    margin-top: var(--space-16);
  }

  .deleted-hint,
  .deleted-problem {
    max-width: 62ch;
    margin: var(--space-6) 0 var(--space-10);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .deleted-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-err-text);
  }

  .deleted-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .deleted-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: var(--space-16);
    padding: var(--space-6) var(--space-6) var(--space-6) var(--space-12);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
  }

  .deleted-name {
    overflow: hidden;
    font-weight: var(--font-weight-medium);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .deleted-when {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 760px) {
    .deleted-row {
      grid-template-columns: minmax(0, 1fr) auto;
      gap: var(--space-2) var(--space-12);
    }

    .deleted-when {
      grid-row: 2;
    }

    .deleted-row :global(.ui-button) {
      grid-row: 1 / span 2;
      grid-column: 2;
    }
  }
</style>
