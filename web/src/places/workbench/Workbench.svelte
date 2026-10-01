<script lang="ts">
  import { tick, untrack } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import { toast } from "../../lib/toast.svelte";
  import type { ArtifactSummary } from "../../lib/types";
  import { Banner, Button, EmptyState, Skeleton } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import ArtifactRow from "./ArtifactRow.svelte";
  import { WorkbenchList } from "./workbench-list.svelte";
  import { artifactRuns } from "./workbench-model";

  // The Workbench: a launcher for the pages agents build. Each opens in its
  // own tab on the artifacts origin, never inside the app. A row selected in
  // the URL opens in place to its link and its running sessions; selecting
  // pushes, switching rows replaces, and Back collapses it.

  let { artifact }: { artifact: string | null } = $props();

  /** How often a reason that may clear by itself (Residuum Cloud connecting) is checked again. */
  const RECHECK_MS = 10_000;

  const list = new WorkbenchList({
    onFrame: (listener) => hub.onFrame(listener),
    page: () => window.location,
  });
  $effect(() => untrack(() => list.start()));

  const blocked = $derived(list.origin !== null && !list.origin.ok ? list.origin.reason : null);
  const selectedRuns = $derived(
    artifact === null ? [] : artifactRuns(overview.overviews, artifact),
  );

  // Relative times move each half minute, and a running session's time each second.
  let now = $state(Date.now());
  $effect(() => {
    const step = selectedRuns.length > 0 ? 1000 : 30_000;
    now = Date.now();
    const timer = window.setInterval(() => {
      now = Date.now();
    }, step);
    return () => window.clearInterval(timer);
  });

  // While artifacts can't open, look again now and then: Residuum Cloud may
  // finish connecting, or the listener come up after a restart.
  $effect(() => {
    if (blocked === null) return;
    const timer = window.setInterval(() => void list.load(), RECHECK_MS);
    return () => window.clearInterval(timer);
  });

  // A fresh read, or a newly selected artifact, settles the URL: an artifact
  // that isn't in the list goes back to the list. The list dropping one this
  // page deleted doesn't, since the delete collapses its row itself.
  $effect(() => {
    const generation = list.generation;
    if (generation === 0 || artifact === null) return;
    untrack(() => {
      router.resolveArtifacts(list.artifacts.map((item) => item.name));
      if (generation === 1) void reveal();
    });
  });

  async function reveal(): Promise<void> {
    if (artifact === null) return;
    await tick();
    document.getElementById(rowId(artifact))?.scrollIntoView({ block: "nearest" });
  }

  function rowId(name: string): string {
    return `workbench-artifact-${name}`;
  }

  function toggle(name: string): void {
    if (artifact === name) void router.closeItem();
    else if (artifact === null) void router.openPlace({ kind: "workbench", artifact: name });
    else void router.replacePlace({ kind: "workbench", artifact: name });
  }

  async function copyLink(item: ArtifactSummary): Promise<void> {
    const url = list.urlOf(item.name);
    if (url === null) return;
    try {
      await navigator.clipboard.writeText(url);
      toast.success(`Copied the link to “${item.title}”.`);
    } catch {
      // Browsers only allow copying on secure pages, which a LAN address over HTTP isn't.
      notifications.surface("error", `Couldn't copy the link. Here it is to copy by hand: ${url}`);
    }
  }

  async function remove(item: ArtifactSummary): Promise<void> {
    const deleted = await list.remove(item);
    if (deleted && artifact === item.name) void router.closeItem();
  }
</script>

<PlaceHeader title="Workbench" sub="Small tools and pages your agents made" />

<div class="bench-scroll">
  <section class="bench" aria-label="Workbench">
    {#if blocked !== null}
      <Banner tone="warn" title="Workbench pages can't open right now.">{blocked}</Banner>
    {/if}

    {#if list.loadError}
      <Banner tone="error">
        {list.loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void list.load()}>Try again</Button>
        {/snippet}
      </Banner>
    {/if}

    {#if !list.loaded}
      {#if list.loadError === null}
        <div class="loading">
          <Skeleton shape="block" height="50px" label="Loading the workbench" />
          <Skeleton shape="block" height="50px" />
        </div>
      {/if}
    {:else if list.artifacts.length === 0}
      <EmptyState variant="block" icon="page" title="Nothing on the bench yet" headingLevel={2}>
        Ask an agent to build you a page: a chart of this month's spending, a calculator for a
        decision you keep revisiting, a dashboard over your inbox. It shows up here as soon as the
        agent writes it, and opens in its own tab.
      </EmptyState>
    {:else}
      <p class="intro">
        Each opens in its own tab, and updates by itself while an agent works on it.
      </p>
      <ul class="rows" aria-label="Pages">
        {#each list.artifacts as item (item.name)}
          <ArtifactRow
            {item}
            {list}
            {now}
            id={rowId(item.name)}
            runs={artifactRuns(overview.overviews, item.name)}
            open={artifact === item.name}
            ontoggle={() => toggle(item.name)}
            oncopy={() => void copyLink(item)}
            onremove={() => void remove(item)}
          />
        {/each}
      </ul>
    {/if}
  </section>
</div>

<style>
  .bench-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .bench {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    width: 100%;
    max-width: calc(var(--layout-reading-width) + var(--space-64));
    margin-inline: auto;
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
    container-type: inline-size;
  }

  .intro {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .loading,
  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .loading {
    gap: var(--space-6);
  }

  @media (max-width: 760px) {
    .bench {
      padding: var(--space-18) var(--space-16) var(--space-40);
    }
  }
</style>
