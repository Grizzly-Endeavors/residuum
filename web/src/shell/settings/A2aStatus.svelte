<script lang="ts">
  import { onMount } from "svelte";
  import { fetchA2aStatus } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { toast } from "../../lib/toast.svelte";
  import type { A2aStatusResponse } from "../../lib/types";
  import { Badge, Banner, Button, IconButton, Skeleton, type BadgeTone } from "../../lib/ui";

  // A running agent's agent-to-agent status: whether the install's listener
  // answers, the address other agents use, how far it reaches, and any
  // problem with the agent's card.

  let { agent }: { agent: string } = $props();

  let status = $state.raw<A2aStatusResponse | null>(null);
  let loadError = $state<string | null>(null);

  async function load(): Promise<void> {
    loadError = null;
    try {
      status = await fetchA2aStatus(agent);
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't check the agent-to-agent status." });
    }
  }

  onMount(() => {
    void load();
  });

  const standing = $derived.by((): { label: string; tone: BadgeTone } | null => {
    if (status === null) return null;
    if (!status.enabled) return { label: "Off for this install", tone: "neutral" };
    if (!status.listener_running) return { label: "Not answering", tone: "danger" };
    return { label: "Listening", tone: "positive" };
  });

  async function copy(address: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(address);
      toast.success("Copied the address.");
    } catch {
      toast.error("Couldn't copy the address. Select it and copy it instead.");
    }
  }
</script>

{#if loadError !== null}
  <Banner tone="error">
    {loadError}
    {#snippet actions()}
      <Button size="sm" onclick={() => void load()}>Try again</Button>
    {/snippet}
  </Banner>
{:else if status === null || standing === null}
  <Skeleton lines={2} label="Loading its status" />
{:else}
  <div class="status">
    <Badge tone={standing.tone} dot>{standing.label}</Badge>
    {#if status.enabled}
      {@const address = status.public_url ?? status.local_url}
      <div class="address">
        <span class="address-label">
          {status.public_url === null
            ? "Its address on this computer"
            : "Its address for other agents"}
        </span>
        <div class="address-row">
          <code class="address-text">{address}</code>
          <IconButton
            icon="copy"
            size="sm"
            label="Copy address"
            onclick={() => void copy(address)}
          />
        </div>
      </div>
      <p class="note">{status.relay_access_note}</p>
      {#if !status.listener_running}
        <Banner tone="warn">
          Nothing is answering on port {status.port} right now. Restart {agent}, or check its logs,
          to find out why.
        </Banner>
      {/if}
    {/if}
    {#if status.card_error}
      <Banner tone="error" title="Its card has a problem.">{status.card_error}</Banner>
    {/if}
  </div>
{/if}

<style>
  .status {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-12);
  }

  .status > :global(.ui-banner) {
    align-self: stretch;
  }

  .address {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
    max-width: 100%;
  }

  .address-label,
  .note {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .address-row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .address-text {
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }
</style>
