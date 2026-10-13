<script lang="ts">
  import { router } from "../../lib/router.svelte";
  import { HOME } from "../../lib/routes";
  import { NOT_FOUND_TITLE } from "../../lib/tab-title";
  import { Button, EmptyState } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";

  // Where an address that leads nowhere in the app lands: it names the
  // address and offers the way back. The rail (or the phone's bottom bar)
  // stays beside it with every agent and place.

  let { path }: { path: string } = $props();

  /** The address as a person typed it: decoded where it can be. */
  const shown = $derived.by(() => {
    try {
      return decodeURIComponent(path);
    } catch {
      return path;
    }
  });
</script>

<PlaceHeader title={NOT_FOUND_TITLE} />

<div class="not-found-scroll">
  <div class="not-found">
    <EmptyState
      variant="block"
      icon="search"
      title="There's nothing at this address"
      headingLevel={2}
    >
      <code class="not-found-path">{shown}</code> isn't a page in Residuum. The link may be
      mistyped, or the page it pointed to may have moved.
      {#snippet actions()}
        <Button variant="primary" icon="home" onclick={() => void router.openPlace(HOME)}>
          Go to Home
        </Button>
        <Button
          icon="inbox"
          onclick={() =>
            void router.openPlace({ kind: "inbox", agent: null, tab: "active", item: null })}
        >
          Open Inbox
        </Button>
      {/snippet}
    </EmptyState>
  </div>
</div>

<style>
  .not-found-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .not-found {
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  .not-found-path {
    overflow-wrap: anywhere;
    color: var(--color-text);
  }
</style>
