<script lang="ts">
  import { onMount } from "svelte";
  import { fetchA2aCard } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import type { A2aAgentCard } from "../../lib/types";
  import { Banner, Button, Skeleton } from "../../lib/ui";

  // The card a running agent serves to agents that reach it, as they see it.

  let { agent }: { agent: string } = $props();

  let card = $state.raw<A2aAgentCard | null>(null);
  let loadError = $state<string | null>(null);

  async function load(): Promise<void> {
    loadError = null;
    try {
      card = await fetchA2aCard(agent);
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't read its card." });
    }
  }

  onMount(() => {
    void load();
  });
</script>

{#if loadError !== null}
  <Banner tone="error">
    {loadError}
    {#snippet actions()}
      <Button size="sm" onclick={() => void load()}>Try again</Button>
    {/snippet}
  </Banner>
{:else if card === null}
  <Skeleton lines={3} label="Loading its card" />
{:else}
  <div class="card">
    <span class="card-name">{card.name}</span>
    {#if card.description}<p class="card-desc">{card.description}</p>{/if}
    {#if card.skills.length === 0}
      <p class="card-empty">It lists no skills yet.</p>
    {:else}
      <ul class="skills" aria-label="Skills it lists">
        {#each card.skills as skill (skill.id)}
          <li>
            <span class="skill-name">{skill.name}</span>
            {#if skill.description}<span class="skill-desc">{skill.description}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
  }

  .card-name {
    font-weight: var(--font-weight-semibold);
  }

  .card-desc {
    color: var(--color-text-2);
  }

  .card-empty,
  .skill-desc {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .skills {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    margin-top: var(--space-4);
    list-style: none;
  }

  .skills li {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .skill-name {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }
</style>
