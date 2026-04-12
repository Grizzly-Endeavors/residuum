<script lang="ts">
  import { onMount } from "svelte";
  import { fetchStatus } from "./lib/api";
  import { ws } from "./lib/ws.svelte";
  import Header from "./components/Header.svelte";
  import BrandMark from "./components/BrandMark.svelte";
  import Chat from "./Chat.svelte";
  import Setup from "./Setup.svelte";
  import Settings from "./Settings.svelte";
  import Workspace from "./components/Workspace.svelte";

  let mode = $state<"loading" | "setup" | "running">("loading");
  let activeView = $state<"chat" | "workspace" | "settings">("chat");

  onMount(async () => {
    try {
      const status = await fetchStatus();
      mode = status.mode === "setup" ? "setup" : "running";
    } catch {
      mode = "running";
    }
  });
</script>

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
