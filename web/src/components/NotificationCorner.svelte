<script lang="ts">
  import { relativeTime } from "../lib/time";
  import { toast } from "../lib/toast.svelte";
  import { notifications } from "../lib/notifications.svelte";
  import { clickOutside } from "../lib/actions/clickOutside";
  import { Icon } from "../lib/icons";

  let open = $state(false);
  let now = $state(Date.now());

  // Tick relative timestamps once a minute, but only while the dropdown
  // is open — no point waking up reactivity for a hidden panel.
  $effect(() => {
    if (!open) return;
    now = Date.now();
    const id = window.setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => window.clearInterval(id);
  });

  function toggle(): void {
    open = !open;
  }

  function close(): void {
    open = false;
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && open) {
      close();
    }
  }

  function clearClicked(): void {
    const cleared = notifications.clear();
    if (cleared.length === 0) return;
    toast.success(`Cleared ${cleared.length} notification(s).`, {
      label: "Undo",
      onClick: () => {
        notifications.restore(cleared);
      },
    });
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="notif-corner">
  <div class="notif-toast-stack" role="status" aria-live="polite">
    {#each [...toast.toasts.values()] as t (t.id)}
      <div class="notif-toast notif-toast-{t.kind}">
        <button
          type="button"
          class="notif-toast-message"
          onclick={() => toast.dismiss(t.id)}
          title="Click to dismiss"
        >
          {t.message}
        </button>
        {#if t.action}
          <button type="button" class="notif-toast-action" onclick={() => toast.runAction(t.id)}>
            {t.action.label}
          </button>
        {/if}
      </div>
    {/each}
  </div>

  <div
    class="notif-recall"
    use:clickOutside={{
      onOutside: () => {
        close();
      },
    }}
  >
    <button
      type="button"
      class="notif-recall-btn"
      class:is-open={open}
      onclick={toggle}
      aria-label="Recent notifications"
      aria-expanded={open}
      aria-haspopup="menu"
      title="Recent notifications"
    >
      <Icon name="pulse" size={16} />
    </button>

    {#if open}
      <div class="notif-dropdown" role="region" aria-label="Recent notifications">
        <div class="notif-dropdown-list">
          {#if notifications.history.length === 0}
            <div class="notif-dropdown-empty">nothing to surface</div>
          {:else}
            {#each notifications.history as item (item.id)}
              <div class="notif-dropdown-item kind-{item.kind}">
                {#if item.details}
                  <details class="notif-dropdown-details">
                    <summary class="notif-dropdown-item-message">{item.message}</summary>
                    <pre class="notif-dropdown-detail-body">{item.details}</pre>
                  </details>
                {:else}
                  <div class="notif-dropdown-item-message">{item.message}</div>
                {/if}
                <div class="notif-dropdown-item-time">{relativeTime(item.timestamp, now)}</div>
              </div>
            {/each}
          {/if}
        </div>
        {#if notifications.history.length > 0}
          <div class="notif-dropdown-footer">
            <button type="button" class="notif-dropdown-clear" onclick={clearClicked}>
              clear all
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>
