<script lang="ts">
  import { userInbox } from "../lib/inbox.svelte";
  import { clickOutside } from "../lib/actions/clickOutside";
  import { Icon } from "../lib/icons";

  let {
    open,
    onClose,
  }: {
    open: boolean;
    onClose: () => void;
  } = $props();

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && open) {
      onClose();
    }
  }

  function handleItemClick(id: string, read: boolean) {
    if (!read) {
      void userInbox.markRead(id);
    }
  }

  function handleArchive(id: string) {
    void userInbox.archive(id);
  }

  function relativeTime(isoString: string): string {
    const then = new Date(isoString);
    const now = new Date();
    const seconds = Math.max(0, Math.round((now.getTime() - then.getTime()) / 1000));
    if (seconds < 45) return "just now";
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.round(hours / 24);
    return `${days}d ago`;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div class="drawer-overlay" aria-hidden="true"></div>
  <div
    class="drawer"
    use:clickOutside={{ onOutside: onClose }}
    role="dialog"
    aria-label="User Inbox"
  >
    <header class="drawer-header">
      <h2 class="drawer-title">Inbox</h2>
      {#if userInbox.unreadCount > 0}
        <span class="drawer-count">{userInbox.unreadCount}</span>
      {/if}
      <button class="drawer-close" onclick={onClose} aria-label="Close">
        <Icon name="close" size={16} />
      </button>
    </header>

    <div class="drawer-content">
      {#if userInbox.items.length === 0}
        <div class="empty-state">
          <p class="empty-state-text">Nothing waiting</p>
        </div>
      {:else}
        <div class="inbox-list">
          {#each userInbox.items as item, i (item.id)}
            <div class="inbox-item" class:unread={!item.read} style="--stagger: {i * 120}ms">
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div class="inbox-item-main" onclick={() => handleItemClick(item.id, item.read)}>
                <div class="inbox-item-header">
                  <span class="inbox-item-title">{item.title}</span>
                  <span class="inbox-item-time">{relativeTime(item.timestamp)}</span>
                </div>
                <span class="inbox-item-source">{item.source}</span>
                {#if item.read}
                  <div class="inbox-item-body">
                    {item.body}
                  </div>
                {:else}
                  <span class="inbox-item-hint">tap to read</span>
                {/if}
              </div>
              <div class="inbox-item-actions">
                <button
                  class="archive-btn"
                  onclick={() => handleArchive(item.id)}
                  title="Archive"
                  aria-label="Archive"
                >
                  <Icon name="check" size={12} />
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <div class="drawer-footer">
      <div class="drawer-footer-vein"></div>
    </div>
  </div>
{/if}

<style>
  .drawer-overlay {
    position: fixed;
    inset: 0;
    background: rgba(14, 14, 16, 0.6);
    backdrop-filter: blur(4px) saturate(0.9);
    -webkit-backdrop-filter: blur(4px) saturate(0.9);
    z-index: 1000;
    animation: overlay-in var(--dur-default) var(--ease-out-stone) forwards;
  }

  @keyframes overlay-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: 380px;
    max-width: 90vw;
    background: var(--bg-deep);
    border-left: 1px solid var(--border-subtle);
    box-shadow: -8px 0 40px rgba(0, 0, 0, 0.45);
    z-index: 1001;
    display: flex;
    flex-direction: column;
    animation: slide-in var(--dur-slow) var(--ease-out-expo) forwards;
  }

  .drawer::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 1px;
    background: linear-gradient(
      180deg,
      transparent 0%,
      var(--vein-subtle) 25%,
      var(--vein) 50%,
      var(--vein-subtle) 75%,
      transparent 100%
    );
    box-shadow: 0 0 10px rgba(59, 139, 219, 0.12);
    pointer-events: none;
  }

  @keyframes slide-in {
    from {
      transform: translateX(100%);
    }
    to {
      transform: translateX(0);
    }
  }

  /* ── Header ─────────────────────────────────────────────────────────── */

  .drawer-header {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: var(--s-5) var(--s-5) var(--s-4);
    position: relative;
  }

  .drawer-header::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 1px;
    background: linear-gradient(
      90deg,
      transparent 0%,
      var(--vein-faint) 20%,
      var(--vein-subtle) 50%,
      var(--vein-faint) 80%,
      transparent 100%
    );
    pointer-events: none;
  }

  .drawer-title {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--fs-lg);
    font-weight: 500;
    letter-spacing: 0.1em;
    color: var(--text);
    text-transform: uppercase;
    text-shadow:
      0 1px 0 rgba(0, 0, 0, 0.6),
      0 -1px 0 rgba(255, 255, 255, 0.03);
  }

  .drawer-count {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-weight: 400;
    color: var(--vein);
    letter-spacing: 0.06em;
    background: var(--vein-faint);
    border: 1px solid var(--vein-dim);
    border-radius: 999px;
    padding: 1px var(--s-2);
    line-height: 1.4;
    text-shadow: 0 0 8px rgba(59, 139, 219, 0.25);
  }

  .drawer-close {
    margin-left: auto;
    background: transparent;
    border: 1px solid var(--border-subtle);
    color: var(--text-dim);
    cursor: pointer;
    width: 30px;
    height: 30px;
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      color var(--dur-quick),
      background-color var(--dur-quick),
      border-color var(--dur-quick);
  }

  .drawer-close:hover {
    color: var(--text);
    background: var(--bg-raised);
    border-color: var(--border);
  }

  .drawer-close:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }

  /* ── Content ─────────────────────────────────────────────────────────── */

  .drawer-content {
    flex: 1;
    overflow-y: auto;
    padding: var(--s-4) var(--s-4) var(--s-5);
  }

  /* ── Empty state ─────────────────────────────────────────────────────── */

  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    min-height: 200px;
  }

  .empty-state-text {
    font-family: var(--font-body);
    font-weight: 300;
    font-style: italic;
    font-size: var(--fs-md);
    color: var(--text-dim);
    margin: 0;
  }

  /* ── Inbox list ──────────────────────────────────────────────────────── */

  .inbox-list {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }

  /* ── Inbox item ──────────────────────────────────────────────────────── */

  .inbox-item {
    position: relative;
    display: flex;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
    overflow: hidden;
    box-shadow: var(--elev-1);
    opacity: 0;
    transform: translateY(8px);
    animation: item-emerge 0.8s var(--ease-out-stone) var(--stagger) forwards;
    transition:
      border-color var(--dur-default) var(--ease-out-stone),
      box-shadow var(--dur-default) var(--ease-out-stone),
      background var(--dur-default) var(--ease-out-stone);
  }

  @keyframes item-emerge {
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .inbox-item:hover {
    border-color: var(--border);
    box-shadow: var(--elev-2);
  }

  .inbox-item.unread {
    border-color: var(--vein-dim);
    background: linear-gradient(100deg, rgba(59, 139, 219, 0.06) 0%, var(--bg-surface) 60%);
  }

  .inbox-item.unread::before {
    content: "";
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    width: 3px;
    background: linear-gradient(
      180deg,
      transparent 0%,
      var(--vein) 15%,
      var(--vein-bright) 50%,
      var(--vein) 85%,
      transparent 100%
    );
    box-shadow: 0 0 6px rgba(59, 139, 219, 0.3);
  }

  .inbox-item-main {
    flex: 1;
    padding: var(--s-4) var(--s-4) var(--s-4) var(--s-5);
    cursor: pointer;
  }

  .inbox-item.unread .inbox-item-main:hover {
    background: rgba(59, 139, 219, 0.04);
  }

  .inbox-item:not(.unread) .inbox-item-main:hover {
    background: var(--bg-raised);
  }

  .inbox-item-header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: var(--s-2);
    margin-bottom: 2px;
  }

  .inbox-item-title {
    font-family: var(--font-body);
    font-weight: 400;
    font-size: var(--fs-body);
    color: var(--text);
    line-height: 1.4;
  }

  .inbox-item.unread .inbox-item-title {
    color: var(--text);
    font-weight: 500;
  }

  .inbox-item:not(.unread) .inbox-item-title {
    color: var(--text-muted);
  }

  .inbox-item-time {
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-weight: 300;
    color: var(--text-dim);
    letter-spacing: 0.04em;
  }

  .inbox-item-source {
    display: block;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-weight: 300;
    color: var(--text-dim);
    letter-spacing: 0.04em;
    text-transform: lowercase;
    margin-bottom: var(--s-2);
  }

  .inbox-item-body {
    font-family: var(--font-body);
    font-weight: 300;
    font-size: var(--fs-md);
    line-height: 1.55;
    color: var(--text-muted);
    white-space: pre-wrap;
    padding-top: var(--s-3);
    border-top: 1px solid var(--border-subtle);
    margin-top: var(--s-2);
  }

  .inbox-item-hint {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    font-weight: 300;
    color: var(--text-dim);
    letter-spacing: 0.06em;
  }

  .inbox-item.unread .inbox-item-hint {
    color: var(--vein-dim);
  }

  /* ── Actions ─────────────────────────────────────────────────────────── */

  .inbox-item-actions {
    display: flex;
    align-items: flex-start;
    padding: var(--s-4) var(--s-3) var(--s-4) 0;
  }

  .archive-btn {
    background: transparent;
    border: 1px solid var(--border-subtle);
    color: var(--text-dim);
    width: 26px;
    height: 26px;
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition:
      color var(--dur-quick),
      background-color var(--dur-quick),
      border-color var(--dur-quick),
      box-shadow var(--dur-default) var(--ease-out-stone);
  }

  .archive-btn:hover {
    color: var(--moss-hover);
    background: var(--moss-faint);
    border-color: var(--moss);
    box-shadow: 0 0 10px -4px rgba(107, 122, 74, 0.4);
  }

  .archive-btn:active {
    transform: scale(0.96);
  }

  .archive-btn:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }

  /* ── Footer vein ─────────────────────────────────────────────────────── */

  .drawer-footer {
    flex-shrink: 0;
    padding: 0 var(--s-4);
  }

  .drawer-footer-vein {
    height: 1px;
    background: linear-gradient(
      90deg,
      transparent 0%,
      var(--vein-faint) 25%,
      var(--vein-subtle) 50%,
      var(--vein-faint) 75%,
      transparent 100%
    );
    box-shadow: 0 0 6px rgba(59, 139, 219, 0.1);
  }

  /* ── Reduced motion ──────────────────────────────────────────────────── */

  @media (prefers-reduced-motion: reduce) {
    .drawer {
      animation: none;
      transform: none;
    }

    .drawer-overlay {
      animation: none;
      opacity: 1;
    }

    .inbox-item {
      animation: none;
      opacity: 1;
      transform: none;
    }
  }
</style>
