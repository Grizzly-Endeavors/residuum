<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon } from "../../lib/icons";
  import { Badge, Button, VisuallyHidden } from "../../lib/ui";
  import type { ChannelState } from "./channel-state";
  import SettingsGroup from "./SettingsGroup.svelte";

  // One place people can talk to the agent: its state beside the title, the
  // fields, and a link to the platform's own instructions.

  interface Props {
    title: string;
    lede: string;
    state: ChannelState | null;
    /** Staged: clears what the channel connects with. Given only while the form holds credentials. */
    ondisconnect?: () => void;
    guide: { href: string; label: string };
    children: Snippet;
  }

  let { title, lede, state, ondisconnect, guide, children }: Props = $props();

  const LABELS: Readonly<Record<ChannelState, string>> = {
    connected: "Connected",
    disconnecting: "Disconnects when saved",
    "not-connected": "Not connected",
  };
</script>

<SettingsGroup {title} {lede}>
  {#snippet status()}
    {#if state !== null}
      <Badge dot tone={state === "connected" ? "positive" : "neutral"}>{LABELS[state]}</Badge>
    {/if}
  {/snippet}
  {#snippet actions()}
    {#if ondisconnect}
      <Button variant="danger" size="sm" aria-label="Disconnect {title}" onclick={ondisconnect}>
        Disconnect
      </Button>
    {/if}
  {/snippet}
  {@render children()}
  <a class="channel-guide" href={guide.href} target="_blank" rel="noopener noreferrer">
    {guide.label}
    <Icon name="external-link" size={13} />
    <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
  </a>
</SettingsGroup>

<style>
  .channel-guide {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    align-self: flex-start;
    font-size: var(--font-size-sm);
  }
</style>
