<script lang="ts">
  import type { Snippet } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { Banner, Button } from "../../lib/ui";

  // A part of a section that needs its agent running (design §8): its
  // agent-to-agent status, its card, whether remote agents can be reached.
  // While the agent isn't running the part says so, with Start, and the rest
  // of the section, which is backed by files, stays editable.

  interface Props {
    agent: string;
    /** What the part shows, for the notice: "its status and card". */
    subject?: string;
    /** The part itself, drawn once the agent runs. */
    children?: Snippet;
  }

  let { agent, subject = "this", children }: Props = $props();

  const agentState = $derived(hub.agent(agent)?.state ?? "stopped");
  const stopping = $derived(hub.isStopping(agent));
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

{#if agentState === "running" && !stopping}
  {@render children?.()}
{:else if agentState === "starting"}
  <Banner busy>Starting {agent}…</Banner>
{:else}
  <Banner>
    Start {agent} to see {subject}.
    {#snippet actions()}
      <Button size="sm" icon="play" loading={starting} disabled={stopping} onclick={start}>
        Start {agent}
      </Button>
    {/snippet}
  </Banner>
{/if}
