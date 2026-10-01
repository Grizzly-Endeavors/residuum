<script lang="ts">
  import { Icon, type IconName } from "../icons";
  import { notifications, type Notification, type NotificationKind } from "../notifications.svelte";
  import { relativeTime } from "../time";
  import { toast } from "../toast.svelte";
  import Button from "./Button.svelte";
  import Dialog from "./Dialog.svelte";
  import Disclosure from "./Disclosure.svelte";
  import EmptyState from "./EmptyState.svelte";
  import VisuallyHidden from "./VisuallyHidden.svelte";

  // The notifications surfaced since the page opened, newest first, with
  // their details and when they came. Clear all offers Undo twice: in the
  // dialog, where the keyboard reaches it, and on the toast it raises.

  let { open = $bindable(false) }: { open?: boolean } = $props();

  const KINDS: Readonly<Record<NotificationKind, { icon: IconName; word: string }>> = {
    error: { icon: "warning", word: "Error" },
    notice: { icon: "info", word: "Notice" },
    system: { icon: "info", word: "Status" },
  };

  let now = $state(Date.now());
  /** The last clear while it can still be undone here: until Undo, or the dialog closes. */
  let lastClear = $state.raw<{ count: number; undo: () => void } | null>(null);

  // Relative times move on once a minute, only while the dialog is open.
  $effect(() => {
    if (!open) {
      lastClear = null;
      return;
    }
    now = Date.now();
    const timer = setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => clearInterval(timer);
  });

  function plural(count: number): string {
    return count === 1 ? "1 notification" : `${String(count)} notifications`;
  }

  function clearAll(): void {
    const entries: Notification[] = notifications.clear();
    if (entries.length === 0) return;
    let undone = false;
    const undo = (): void => {
      if (undone) return;
      undone = true;
      notifications.restore(entries);
      toast.dismiss(toastId);
      if (lastClear?.undo === undo) lastClear = null;
    };
    const toastId = toast.success(`Cleared ${plural(entries.length)}.`, {
      label: "Undo",
      onClick: undo,
    });
    lastClear = { count: entries.length, undo };
  }

  const history = $derived(notifications.history);
</script>

<Dialog
  bind:open
  title="Recent notifications"
  description="Errors and notices since this page opened."
  size="md"
  fullscreenOnPhone
  actions={history.length > 0 || lastClear !== null ? footer : undefined}
>
  {#if history.length > 0}
    <ol class="ui-notifications">
      {#each history as item (item.id)}
        <li class="ui-notification" data-kind={item.kind}>
          <span class="ui-notification-icon"><Icon name={KINDS[item.kind].icon} size={15} /></span>
          <div class="ui-notification-body">
            <p class="ui-notification-message">
              <VisuallyHidden>{KINDS[item.kind].word}:</VisuallyHidden>
              {item.message}
            </p>
            {#if item.details}
              <Disclosure summary="Details" tone="quiet">
                <pre class="ui-notification-details">{item.details}</pre>
              </Disclosure>
            {/if}
          </div>
          <time
            class="ui-notification-time"
            datetime={item.timestamp.toISOString()}
            title={item.timestamp.toLocaleString()}
          >
            {relativeTime(item.timestamp, now)}
          </time>
        </li>
      {/each}
    </ol>
  {:else if lastClear !== null}
    <EmptyState>Cleared {plural(lastClear.count)}.</EmptyState>
  {:else}
    <EmptyState
      >Nothing yet. Errors and notices from Residuum and your agents show up here.</EmptyState
    >
  {/if}
</Dialog>

<!-- One button that changes its job, so focus stays on it from Clear all to Undo and back. -->
{#snippet footer()}
  <Button
    variant="quiet"
    icon={history.length === 0 ? "restore" : undefined}
    onclick={() => {
      if (history.length === 0) lastClear?.undo();
      else clearAll();
    }}
  >
    {history.length === 0 ? "Undo" : "Clear all"}
  </Button>
{/snippet}

<style>
  .ui-notifications {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    list-style: none;
  }

  .ui-notification {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: var(--space-10);
    align-items: start;
    padding: var(--space-6) 0;
  }

  .ui-notification-icon {
    display: grid;
    place-items: center;
    height: 20px;
    color: var(--color-text-2);

    [data-kind="error"] > & {
      color: var(--color-err-text);
    }

    [data-kind="notice"] > & {
      color: var(--color-vein-bright);
    }
  }

  .ui-notification-body {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-4);
    min-width: 0;
  }

  .ui-notification-message {
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  .ui-notification-details {
    margin-top: var(--space-4);
    padding: var(--space-8) var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .ui-notification-time {
    padding-top: var(--space-2);
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
</style>
