<script lang="ts">
  import type { ConnectionStatus } from "../lib/types";

  let {
    status,
    activeView,
    onOpenChat,
    onOpenWorkspace,
    onOpenSettings,
  }: {
    status: ConnectionStatus;
    activeView: "chat" | "workspace" | "settings";
    onOpenChat: () => void;
    onOpenWorkspace: () => void;
    onOpenSettings: () => void;
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
    <button class="hamburger-btn" onclick={toggleMenu} title="Menu">&#9776;</button>
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
    <div class="header-icon">&#9670;</div>
    <span class="header-title">Residuum</span>
    <span class="header-status {status}">{status}</span>
  </div>
</div>
