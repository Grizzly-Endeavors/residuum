<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { router } from "../lib/router.svelte";

  let { onClose }: { onClose: () => void } = $props();
</script>

<section class="team-overview">
  <header class="team-overview-head">
    <h2>Team</h2>
    <button type="button" onclick={onClose}>Close</button>
  </header>
  {#if hub.agents.length === 0}
    <p>No agents.</p>
  {:else}
    <ul class="team-overview-list">
      {#each hub.agents as agent (agent.name)}
        <li>
          <button type="button" onclick={() => router.openAgent(agent.name)}>{agent.name}</button>
          <span>{agent.state}</span>
          {#if agent.last_error}
            <span role="alert">{agent.last_error.message}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .team-overview {
    padding: var(--s-4);
  }

  .team-overview-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .team-overview-list {
    list-style: none;
    padding: 0;
  }

  .team-overview-list li {
    display: flex;
    gap: var(--s-3);
    align-items: center;
    padding: var(--s-2) 0;
  }
</style>
