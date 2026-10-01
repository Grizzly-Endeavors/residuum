<script lang="ts">
  import { untrack } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import type { HubInboxItem } from "../../lib/hub-types";
  import { inbox, inboxItemKey } from "../../lib/inbox.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import type { InboxItemRef, InboxTab, Place } from "../../lib/routes";
  import { Banner, Button, EmptyState, SelectField, Skeleton, Tabs } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import InboxItem from "./InboxItem.svelte";

  // The Inbox: every agent's user inbox in one list, newest first, filtered
  // to one agent or not, with the archive on its own tab. The filter, the tab
  // and the open item live in the URL; opening an item pushes, everything
  // else here replaces.

  type InboxPlace = Extract<Place, { kind: "inbox" }>;

  let { place }: { place: InboxPlace } = $props();

  const ALL = "";
  const agent = $derived(place.agent);
  const tab = $derived(place.tab);
  const openKey = $derived(place.item === null ? null : inboxItemKey(place.item));

  /** The open item when the list doesn't hold it. */
  const apart = $derived.by(() => {
    const item = inbox.openedApart;
    if (item === null || inboxItemKey(item) !== openKey) return null;
    return inbox.items.some((held) => inboxItemKey(held) === openKey) ? null : item;
  });

  /** Unread items, for everyone or for the filtered agent. */
  const unread = $derived(
    agent === null ? overview.inboxUnread : (overview.overviewOf(agent)?.inbox_unread ?? 0),
  );

  const filterOptions = $derived([
    { value: ALL, label: "All agents" },
    ...hub.agents.map((summary) => {
      const count = overview.overviewOf(summary.name)?.inbox_unread ?? 0;
      return {
        value: summary.name,
        label:
          tab === "active" && count > 0
            ? `${summary.name} (${String(count)} unread)`
            : summary.name,
      };
    }),
  ]);

  /** The clock relative times read, moving each half minute. */
  let now = $state(Date.now());
  $effect(() => {
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => window.clearInterval(timer);
  });

  // The list follows the filter and the tab, and is fetched each time the Inbox opens.
  $effect(() => {
    const list = { agent, tab };
    untrack(() => void inbox.show(list));
  });

  // New items arrive as the overview's counts change.
  $effect(() => {
    const counts = overview.unreadCounts();
    untrack(() => {
      inbox.followCounts(counts);
    });
  });
  $effect(() => () => {
    inbox.leave();
  });

  // Opening an item, from here or from a link, marks it read.
  $effect(() => {
    if (openKey === null) return;
    untrack(() => {
      if (place.item !== null) void reveal(place.item);
    });
  });

  async function reveal(ref: InboxItemRef): Promise<void> {
    const outcome = await inbox.open(ref);
    const current = router.place;
    const stillOpen =
      current.kind === "inbox" &&
      current.item !== null &&
      inboxItemKey(current.item) === inboxItemKey(ref);
    if (!stillOpen) return;
    if (outcome === "missing") {
      notifications.surface("notice", `That item isn't in ${ref.agent}'s inbox any more.`);
      void router.replacePlace({ ...current, item: null });
      return;
    }
    document.getElementById(rowId(ref))?.scrollIntoView({ block: "nearest" });
  }

  function rowId(ref: InboxItemRef): string {
    return `inbox-item-${inboxItemKey(ref)}`;
  }

  /** Open an item, close it, or switch to it from the one that is open. */
  function toggle(item: HubInboxItem): void {
    const ref: InboxItemRef = { agent: item.agent, id: item.id };
    if (inboxItemKey(ref) === openKey) void router.closeInboxItem();
    else if (place.item === null) void router.openPlace({ ...place, item: ref });
    else void router.replacePlace({ ...place, item: ref });
  }

  function showList(next: { agent?: string | null; tab?: InboxTab }): void {
    void router.replacePlace({
      kind: "inbox",
      agent: next.agent === undefined ? agent : next.agent,
      tab: next.tab ?? tab,
      item: null,
    });
  }

  const TABS = $derived([
    { value: "active" as const, label: "Inbox", count: unread },
    { value: "archived" as const, label: "Archived" },
  ]);
</script>

<PlaceHeader title="Inbox" sub="Things your agents want you to see" />

<div class="inbox-scroll">
  <section class="inbox" aria-label="Inbox">
    <div class="inbox-filter">
      <SelectField
        label="Show items from"
        labelHidden
        value={agent ?? ALL}
        options={filterOptions}
        onchange={(event) => showList({ agent: event.currentTarget.value || null })}
      />
    </div>

    <Tabs
      label="Inbox or archive"
      tabs={TABS}
      selected={tab}
      onchange={(next) => showList({ tab: next })}
    >
      {#snippet children(shown)}
        {#if inbox.openError}
          <Banner tone="error">
            {inbox.openError}
            {#snippet actions()}
              {#if place.item}
                {@const ref = place.item}
                <Button variant="quiet" size="sm" onclick={() => void reveal(ref)}>Try again</Button
                >
              {/if}
            {/snippet}
          </Banner>
        {/if}

        {#if apart}
          <ul class="inbox-list inbox-apart" aria-label="Opened item">
            {@render row(apart)}
          </ul>
        {/if}

        {#if inbox.loadError}
          <Banner tone="error">
            {inbox.loadError}
            {#snippet actions()}
              <Button variant="quiet" size="sm" onclick={() => void inbox.reload()}
                >Try again</Button
              >
            {/snippet}
          </Banner>
        {:else if !inbox.loaded}
          <div class="inbox-loading">
            <Skeleton shape="block" height="50px" label="Loading your inbox" />
            <Skeleton shape="block" height="50px" />
            <Skeleton shape="block" height="50px" />
          </div>
        {:else if inbox.items.length === 0}
          <EmptyState>
            {#if shown === "archived" && agent === null}
              Nothing is archived. Archived items wait here until you move them back.
            {:else if shown === "archived"}
              Nothing from {agent} is archived.
            {:else if agent === null}
              You're all caught up. Agents put things here when they need you to see them.
            {:else}
              Nothing from {agent} right now.
            {/if}
          </EmptyState>
        {:else}
          <ul class="inbox-list" aria-label={shown === "archived" ? "Archived items" : "Items"}>
            {#each inbox.items as item (inboxItemKey(item))}
              {@render row(item)}
            {/each}
          </ul>
          {#if inbox.moreError}
            <p class="inbox-problem" role="alert">
              {inbox.moreError}
              <Button variant="quiet" size="sm" onclick={() => void inbox.loadMore()}
                >Try again</Button
              >
            </p>
          {:else if inbox.nextCursor !== null}
            <div class="inbox-more">
              <Button
                variant="quiet"
                size="sm"
                loading={inbox.loadingMore}
                onclick={() => void inbox.loadMore()}>Show older items</Button
              >
            </div>
          {/if}
        {/if}
      {/snippet}
    </Tabs>
  </section>
</div>

{#snippet row(item: HubInboxItem)}
  <InboxItem
    {item}
    {now}
    id={rowId(item)}
    open={inboxItemKey(item) === openKey}
    ontoggle={() => toggle(item)}
    onleave={() => void router.closeInboxItem()}
  />
{/snippet}

<style>
  .inbox-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .inbox {
    position: relative;
    width: 100%;
    max-width: calc(var(--layout-reading-width) + var(--space-64));
    margin-inline: auto;
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
    container-type: inline-size;
  }

  /* Beside the tabs while there's room, centered on them; above them when there isn't. */
  .inbox-filter {
    position: absolute;
    top: calc(var(--space-24) - 3px);
    right: clamp(var(--space-16), 3vw, var(--space-32));
    width: 200px;
  }

  @container (max-width: 520px) {
    .inbox-filter {
      position: static;
      width: 100%;
      margin-bottom: var(--space-12);
    }
  }

  .inbox-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .inbox-apart {
    margin-bottom: var(--space-16);
  }

  .inbox-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .inbox-more {
    margin-top: var(--space-12);
  }

  .inbox-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    margin-top: var(--space-12);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  @media (max-width: 760px) {
    .inbox {
      padding: var(--space-18) var(--space-16) var(--space-40);
    }
  }
</style>
