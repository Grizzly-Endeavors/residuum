<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { router } from "../lib/router.svelte";

  function label(name: string, state: string, busy: boolean, unread: number): string {
    const extras = [busy ? "working" : "", unread > 0 ? `${unread} unread` : ""].filter(Boolean);
    return extras.length > 0 ? `${name} · ${state} · ${extras.join(", ")}` : `${name} · ${state}`;
  }

  // The agent in the URL may not be in the list yet (or ever); show it anyway
  // so the picker never claims a different agent than the page is on.
  let listed = $derived(hub.agents.some((a) => a.name === router.agent));
</script>

<select
  class="agent-picker"
  aria-label="Agent"
  value={router.agent ?? ""}
  onchange={(event) => router.openAgent(event.currentTarget.value)}
>
  {#if router.agent !== null && !listed}
    <option value={router.agent}>{router.agent}</option>
  {/if}
  {#each hub.agents as agent (agent.name)}
    {@const { busy, unread } = hub.activityOf(agent.name)}
    <option value={agent.name}>{label(agent.name, agent.state, busy, unread)}</option>
  {/each}
</select>

<style>
  .agent-picker {
    max-width: 16rem;
    background: transparent;
    color: var(--text);
    border: 1px solid var(--vein-dim);
    border-radius: var(--radius-sm);
    padding: 0.25rem 0.5rem;
    font-family: inherit;
  }
</style>
