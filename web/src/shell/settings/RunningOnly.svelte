<script lang="ts">
  import type { Snippet } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { Banner, Button } from "../../lib/ui";

  interface Props {
    agent: string;
    subject?: string;
    children?: Snippet;
  }

  let { agent, subject = "this", children }: Props = $props();

  const agentState = $derived(hub.displayStateOf(agent) ?? "stopped");
  let starting = $state(false);

  async function start(): Promise<void> {
    starting = true;
    try {
      await hub.startAgent(agent);
    } finally {
      starting = false;
    }
  }
</script>

{#if agentState === "running"}
  {@render children?.()}
{:else if agentState === "starting"}
  <Banner busy>Starting {agent}…</Banner>
{:else}
  <Banner>
    Start {agent} to see {subject}.
    {#snippet actions()}
      <Button
        size="sm"
        icon="play"
        loading={starting}
        disabled={agentState === "stopping"}
        onclick={start}
      >
        Start {agent}
      </Button>
    {/snippet}
  </Banner>
{/if}
