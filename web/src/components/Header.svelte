<script lang="ts">
  import type { ConnectionStatus } from "../lib/types";
  import { Icon } from "../lib/icons";
  import BrandMark from "./BrandMark.svelte";
  import { userInbox } from "../lib/inbox.svelte";

  let {
    status,
    activeView,
    onOpenChat,
    onOpenWorkspace,
    onOpenSettings,
    onOpenFeedback,
    onOpenInbox,
  }: {
    status: ConnectionStatus;
    activeView: "chat" | "workspace" | "settings";
    onOpenChat: () => void;
    onOpenWorkspace: () => void;
    onOpenSettings: () => void;
    onOpenFeedback: () => void;
    onOpenInbox: () => void;
  } = $props();

  let menuOpen = $state(false);

  function toggleMenu() {
    menuOpen = !menuOpen;
  }

  function select(action: () => void) {
    action();
    menuOpen = false;
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest(".hamburger-wrap")) {
      menuOpen = false;
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div class="header">
  <div class="hamburger-wrap">
    <button class="hamburger-btn" onclick={toggleMenu} title="Menu" aria-label="Menu">
      <Icon name="menu" size={18} />
    </button>
    {#if menuOpen}
      <div class="hamburger-menu">
        <button
          class="hamburger-menu-item"
          class:active={activeView === "chat"}
          onclick={() => select(onOpenChat)}>Chat</button
        >
        <button
          class="hamburger-menu-item"
          class:active={activeView === "workspace"}
          onclick={() => select(onOpenWorkspace)}>Workspace</button
        >
        <button
          class="hamburger-menu-item"
          class:active={activeView === "settings"}
          onclick={() => select(onOpenSettings)}>Settings</button
        >
      </div>
    {/if}
  </div>
  <div class="header-brand">
    <BrandMark size={26} />
    <span class="header-title">Residuum</span>
    <span class="header-status {status}">{status}</span>
  </div>
  <div class="header-right">
    <button
      class="header-action-btn"
      onclick={onOpenInbox}
      title="User Inbox"
      aria-label="User Inbox"
    >
      <Icon name="inbox" size={16} />
      {#if userInbox.unreadCount > 0}
        <span class="badge">{userInbox.unreadCount}</span>
      {/if}
    </button>
    <button
      class="header-action-btn"
      onclick={onOpenFeedback}
      title="Report a bug or send feedback"
      aria-label="Report a bug or send feedback"
    >
      <Icon name="bug" size={16} />
    </button>
  </div>
</div>

<style>
  .header-right {
    display: flex;
    gap: 0.25rem;
    align-items: center;
  }

  .header-action-btn {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-dim);
    cursor: pointer;
    transition:
      color var(--dur-default) var(--ease-out-stone),
      background-color var(--dur-default) var(--ease-out-stone),
      border-color var(--dur-default) var(--ease-out-stone),
      box-shadow var(--dur-default) var(--ease-out-stone);
  }

  .header-action-btn:hover {
    color: var(--vein-bright);
    background: var(--vein-faint);
    border-color: var(--vein-dim);
    box-shadow: 0 0 12px -4px rgba(59, 139, 219, 0.45);
  }

  .badge {
    position: absolute;
    top: 2px;
    right: 2px;
    background: var(--error-bright);
    color: #fff;
    font-size: 0.6rem;
    font-weight: bold;
    min-width: 14px;
    height: 14px;
    border-radius: 7px;
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
    padding: 0 3px;
  }
</style>
