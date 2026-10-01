<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { hub } from "./lib/hub.svelte";
  import { notifications } from "./lib/notifications.svelte";
  import { userErrorMessage } from "./lib/errors";
  import { router } from "./lib/router.svelte";
  import { Icon } from "./lib/icons";
  import {
    provideTooltips,
    Spinner,
    ToastRegion,
    tooltip,
    TooltipHost,
    VisuallyHidden,
  } from "./lib/ui";
  import Setup from "./Setup.svelte";
  import Shell from "./shell/Shell.svelte";

  // The app's root: first-run setup when the hub has no agents, else the
  // shell. Toasts and tooltips are drawn here in every mode.

  provideTooltips(tooltip);

  let mode = $state<"loading" | "setup" | "running">("loading");

  router.start();

  /** On phones, toasts clear the bottom bar, and the composer too on a place or panel that has one. */
  const toastClearance = $derived.by(() => {
    if (mode !== "running") return "edge";
    const { place, panel } = router;
    const composer = panel === null ? place.kind === "chat" : panel.kind === "session";
    return composer ? "composer" : "bar";
  });

  // No agents yet means first-run setup. The hub socket stays up for the life
  // of the page and feeds the rail; the agent connection follows the router's
  // bound agent (see `ws.svelte.ts`).
  onMount(() => {
    hub.connect();
    void (async () => {
      try {
        await hub.refresh(true);
        mode = hub.agents.length === 0 ? "setup" : "running";
      } catch (err) {
        notifications.surface(
          "error",
          userErrorMessage(err, { action: "Couldn't load your agents." }),
        );
        mode = "running";
      }
    })();
    return () => {
      hub.disconnect();
      ws.disconnect();
    };
  });

  // Settle on agents that exist: a URL on an agent that doesn't goes to Home,
  // and one that resolves under the last-used agent finds it.
  $effect(() => {
    if (mode !== "running" || !hub.loaded) return;
    const names = hub.agents.map((agent) => agent.name);
    untrack(() => router.setKnownAgents(names));
  });

  async function finishSetup(): Promise<void> {
    try {
      await hub.refresh(true);
    } catch {
      // the hub socket delivers the list once it connects
    }
    mode = "running";
  }
</script>

{#if mode === "loading"}
  <div class="app-loading" data-ui role="status">
    <span class="app-loading-mark"><Icon name="mark" size={18} />Residuum</span>
    <Spinner size={16} />
    <VisuallyHidden>Loading your agents</VisuallyHidden>
  </div>
{:else if mode === "setup"}
  <Setup onComplete={() => void finishSetup()} />
{:else}
  <Shell />
{/if}

<ToastRegion clearance={toastClearance} />
<TooltipHost />

<style>
  .app-loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-16);
    height: 100%;
    color: var(--color-text-3);
  }

  .app-loading-mark {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    color: var(--color-text);
    font-family: var(--font-mark);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    letter-spacing: 0.2em;
    text-transform: uppercase;

    & :global(svg) {
      color: var(--color-vein);
    }
  }
</style>
