<script lang="ts">
  import { onMount } from "svelte";
  import { fetchStatus } from "./lib/api";
  import { ws } from "./lib/ws.svelte";
  import Header from "./components/Header.svelte";
  import BrandMark from "./components/BrandMark.svelte";
  import NotificationCorner from "./components/NotificationCorner.svelte";
  import HelpOverlay from "./components/HelpOverlay.svelte";
  import Chat from "./Chat.svelte";
  import Setup from "./Setup.svelte";
  import Settings from "./Settings.svelte";
  import Workspace from "./components/Workspace.svelte";

  let mode = $state<"loading" | "setup" | "running">("loading");
  let activeView = $state<"chat" | "workspace" | "settings">("chat");
  let helpOpen = $state(false);

  onMount(async () => {
    try {
      const status = await fetchStatus();
      mode = status.mode === "setup" ? "setup" : "running";
    } catch {
      mode = "running";
    }
  });

  function handleKeydown(event: KeyboardEvent) {
    // `?` opens help — but only when nothing else is taking text input.
    if (event.key !== "?") return;
    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable)
      return;
    event.preventDefault();
    helpOpen = true;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if mode === "loading"}
  <div class="header">
    <div class="header-brand">
      <BrandMark size={26} />
      <span class="header-title">Residuum</span>
      <span class="header-status connecting">loading</span>
    </div>
  </div>
{:else if mode === "setup"}
  <div class="header">
    <div class="header-brand">
      <BrandMark size={26} />
      <span class="header-title">Residuum</span>
    </div>
  </div>
  <Setup
    onComplete={() => {
      mode = "running";
    }}
  />
{:else}
  <Header
    status={ws.transport.status}
    {activeView}
    onOpenChat={() => {
      activeView = "chat";
    }}
    onOpenWorkspace={() => {
      activeView = activeView === "workspace" ? "chat" : "workspace";
    }}
    onOpenSettings={() => {
      activeView = activeView === "settings" ? "chat" : "settings";
    }}
  />
  {#if activeView === "settings"}
    <Settings
      onClose={() => {
        activeView = "chat";
      }}
    />
  {:else}
    <div class="app-main emerges" class:with-workspace={activeView === "workspace"}>
      {#if activeView === "workspace"}
        <Workspace
          onClose={() => {
            activeView = "chat";
          }}
        />
      {/if}
      <Chat />
    </div>
  {/if}
{/if}

<NotificationCorner />
<HelpOverlay
  open={helpOpen}
  onClose={() => {
    helpOpen = false;
  }}
/>
