<script lang="ts">
  import { onMount } from "svelte";
  import { fetchStatus } from "./lib/api";
  import { ws } from "./lib/ws.svelte";
  import Header from "./components/Header.svelte";
  import Chat from "./Chat.svelte";
  import Setup from "./Setup.svelte";

  let mode = $state<"loading" | "setup" | "running">("loading");

  onMount(async () => {
    try {
      const status = await fetchStatus();
      mode = status.mode === "setup" ? "setup" : "running";
    } catch {
      // If status check fails, assume running (server may not support setup)
      mode = "running";
    }
  });

  function handleToggleVerbose() {
    ws.setVerbose(!ws.verbose);
  }
</script>

{#if mode === "loading"}
  <div class="header">
    <div class="header-brand">
      <div class="header-icon">&#9670;</div>
      <span class="header-title">Residuum</span>
      <span class="header-status connecting">loading</span>
    </div>
  </div>
{:else if mode === "setup"}
  <div class="header">
    <div class="header-brand">
      <div class="header-icon">&#9670;</div>
      <span class="header-title">Residuum</span>
    </div>
  </div>
  <Setup onComplete={() => { mode = "running"; }} />
{:else}
  <Header
    status={ws.status}
    verbose={ws.verbose}
    onToggleVerbose={handleToggleVerbose}
  />
  <Chat />
{/if}
