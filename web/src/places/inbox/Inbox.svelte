<script lang="ts">
  import { tick, untrack } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import type { MailFolder } from "../../lib/hub-types";
  import { mail } from "../../lib/mail.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router, type PlaceExtras } from "../../lib/router.svelte";
  import { formatLocation, locationAt, type InboxPlace } from "../../lib/routes";
  import { Badge, Banner, Button, EmptyState, SelectField, Skeleton } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import MailCompose from "./MailCompose.svelte";
  import MailThread from "./MailThread.svelte";
  import MailThreadRow from "./MailThreadRow.svelte";
  import { emptyListText, FOLDERS, panelOfThread } from "./mail-model";

  // The Inbox: mail between the user and their agents, one folder at a time
  // (Inbox, Sent, Archived), for every agent or one. The list sits beside the
  // open thread or New message while there is room for both, and gives way to
  // it when there isn't. The folder, the filter, the open thread and the New
  // message live in the URL. Opening a thread or a New message from the list
  // pushes, so Back closes it; switching threads, folders and the filter
  // replace. A file panel that shows one of the open thread's attachments
  // belongs to the thread and closes with it. Focus follows what opens: the
  // pane's heading when the list made way for it or a message was sent, the
  // row (or the list) when it closes.

  let { place }: { place: InboxPlace } = $props();

  const ALL = "";
  const folder = $derived(place.folder);
  const agent = $derived(place.agent);
  const composing = $derived(place.compose !== null);
  /** The ref of the open thread; a New message closes it. */
  const ref = $derived(place.compose === null ? place.thread : null);
  const itemOpen = $derived(composing || ref !== null);
  const missing = $derived(ref !== null && mail.openRef === ref && mail.threadStatus === "missing");
  /** The open thread's own id once loaded, else the ref it was asked for by. */
  const openId = $derived(
    mail.thread !== null && mail.openRef === ref ? mail.thread.thread_id : ref,
  );

  /** Unread items, for everyone or for the filtered agent, from the overview. */
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
          folder === "inbox" && count > 0
            ? `${summary.name} (${String(count)} unread)`
            : summary.name,
      };
    }),
  ]);

  let listElement = $state<HTMLElement>();
  let paneElement = $state<HTMLElement>();

  /** The clock relative times read, moving each half minute. */
  let now = $state(Date.now());
  $effect(() => {
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => window.clearInterval(timer);
  });

  // The list follows the folder and the filter, and is fetched each time the Inbox opens.
  $effect(() => {
    const list = { folder, agent };
    untrack(() => void mail.show(list));
  });

  // New mail arrives as the overview's counts change.
  $effect(() => {
    const counts = overview.unreadCounts();
    untrack(() => {
      mail.followCounts(counts);
    });
  });
  // Leaving the Inbox ends what it held open. The guard has been answered by
  // then, so an unsent New message goes with it rather than greeting the next one.
  $effect(() => () => {
    mail.leave();
    mail.close();
    mail.discardNewMessage();
  });

  // Leaving with an unsent New message, or closing the page with an unsent
  // reply, asks first.
  $effect(() => router.guard.register((target) => mail.draftLoss(target)));

  // The thread in the URL is open, from here or from a link.
  $effect(() => {
    const opened = ref;
    untrack(() => {
      if (opened === null) mail.close();
      else void reveal(opened);
    });
  });

  // A New message in the URL is the store's draft; one that closes without
  // sending (Cancel, Back, or leaving once the guard was answered) is dropped.
  $effect(() => {
    const open = composing;
    untrack(() => {
      if (open && place.compose !== null) mail.openNewMessage(place.compose.to);
      else mail.discardNewMessage();
    });
  });

  async function reveal(asked: string): Promise<void> {
    const outcome = await mail.open(asked);
    if (outcome.status !== "shown") return;
    const current = router.place;
    if (current.kind !== "inbox" || current.compose !== null || current.thread !== asked) return;
    // Any message's ref opens its thread; the address names the thread itself.
    if (outcome.threadId !== asked) {
      const panel = router.panel;
      void router.replacePlace(
        { ...current, thread: outcome.threadId },
        panel === null ? {} : { panel },
      );
    }
    await tick();
    document.getElementById(rowId(outcome.threadId))?.scrollIntoView({ block: "nearest" });
  }

  function rowId(threadId: string): string {
    return `mail-row-${threadId}`;
  }

  /** The panel to carry along when the open thread changes: any but one of its attachments. */
  function panelKept(): PlaceExtras {
    const panel = router.panel;
    return panel === null || panelOfThread(mail.thread, panel) ? {} : { panel };
  }

  /** A plain click opens the place in the app; one asking for a new tab is left to the browser. */
  function followsInPlace(event: MouseEvent): boolean {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
      return false;
    }
    event.preventDefault();
    return true;
  }

  function threadPlace(threadId: string): InboxPlace {
    return { ...place, thread: threadId, compose: null };
  }

  async function openThread(threadId: string): Promise<void> {
    if (threadId === openId && !composing) return;
    const next = threadPlace(threadId);
    const moved = itemOpen
      ? await router.replacePlace(next, panelKept())
      : await router.openPlace(next, panelKept());
    if (moved) await focusPaneWhenAlone();
  }

  function newMessage(): void {
    if (composing) return;
    const next: InboxPlace = {
      ...place,
      thread: null,
      compose: { to: agent === null ? [] : [agent] },
    };
    const go = itemOpen
      ? router.replacePlace(next, panelKept())
      : router.openPlace(next, panelKept());
    void go.then((moved) => (moved ? focusPaneWhenAlone() : undefined));
  }

  function listPlace(next: { folder?: MailFolder; agent?: string | null }): InboxPlace {
    return {
      ...place,
      folder: next.folder ?? folder,
      agent: next.agent === undefined ? agent : next.agent,
      thread: null,
      compose: null,
    };
  }

  function showList(next: { folder?: MailFolder; agent?: string | null }): void {
    void router.replacePlace(listPlace(next), panelKept());
  }

  /** Back to the list, closing the panel when it shows one of the thread's attachments. */
  async function backToList(): Promise<void> {
    const left = openId;
    if (panelOfThread(mail.thread, router.panel) && !(await router.closePanel())) return;
    if (!(await router.closeItem())) return;
    await tick();
    const row = left === null ? null : document.getElementById(rowId(left));
    (row ?? listElement)?.focus();
  }

  /** The message went: open its thread in Sent, where focus lands on its heading. */
  async function sent(threadId: string): Promise<void> {
    const moved = await router.replacePlace(
      { ...place, folder: "sent", thread: threadId, compose: null },
      panelKept(),
    );
    if (moved) await focusPane();
  }

  /** Focus is owed to the pane's heading: something opened or replaced what it shows. */
  let focusOwed = false;

  /** The list has made way for the pane (below the split width one shows at a time). */
  function listHidden(): boolean {
    return listElement !== undefined && getComputedStyle(listElement).display === "none";
  }

  /** Nothing the user chose holds focus: it was dropped, is in the pane already, or is in a list now hidden. */
  function focusIsLoose(): boolean {
    const active = document.activeElement;
    if (active === null || active === document.body) return true;
    if (paneElement?.contains(active)) return true;
    return listElement?.contains(active) === true && listHidden();
  }

  /**
   * Move focus into the pane: to the open item's heading (the subject, or
   * "New message"), and until a thread has loaded to its named region.
   */
  async function focusPane(): Promise<void> {
    focusOwed = true;
    await tick();
    settleFocus(true);
  }

  /**
   * Pay the focus owed: at once for the action that owes it, and again as
   * the pane's content arrives, unless the user has moved focus elsewhere
   * by then.
   */
  function settleFocus(atOnce = false): void {
    if (!itemOpen) focusOwed = false;
    if (!focusOwed || paneElement === undefined) return;
    if (!atOnce && !focusIsLoose()) {
      focusOwed = false;
      return;
    }
    const heading = paneElement.querySelector<HTMLElement>("h2[tabindex='-1']");
    if (heading !== null) {
      heading.focus();
      focusOwed = false;
      return;
    }
    paneElement.querySelector<HTMLElement>("section[tabindex='-1']")?.focus();
    if (missing || mail.threadStatus === "error") focusOwed = false;
  }

  // A thread's heading arrives after the thread loads.
  $effect(() => {
    void mail.thread;
    void mail.threadStatus;
    void itemOpen;
    untrack(() => {
      settleFocus();
    });
  });

  /** When the list made way for what opened, focus follows it there instead of being dropped. */
  async function focusPaneWhenAlone(): Promise<void> {
    await tick();
    if (listHidden()) await focusPane();
  }

  /**
   * Page older threads in. The button leaves with the last page, so when it
   * had focus, the first new row takes it (the list, if there is none).
   */
  async function showOlder(event: MouseEvent & { currentTarget: HTMLElement }): Promise<void> {
    const button = event.currentTarget;
    const focused = button === document.activeElement;
    const held = mail.threads.length;
    await mail.loadMore();
    await tick();
    if (!focused || button.isConnected) return;
    const first = mail.threads[held];
    (first === undefined ? listElement : document.getElementById(rowId(first.thread_id)))?.focus();
  }
</script>

<PlaceHeader title="Inbox" sub="Mail between you and your agents">
  <span class="mail-header-actions">
    <Button variant="primary" icon="edit" onclick={newMessage}>New message</Button>
  </span>
</PlaceHeader>

<div class="mail" data-open={itemOpen || undefined}>
  <section class="mail-list" aria-label="Conversations" tabindex="-1" bind:this={listElement}>
    <div class="mail-tools">
      <nav class="mail-folders" aria-label="Folders">
        {#each FOLDERS as entry (entry.folder)}
          <a
            class="mail-folder"
            href={formatLocation(locationAt(listPlace({ folder: entry.folder })))}
            aria-current={entry.folder === folder ? "page" : undefined}
            onclick={(event) => {
              if (followsInPlace(event) && entry.folder !== folder)
                showList({ folder: entry.folder });
            }}
          >
            {entry.label}
            {#if entry.folder === "inbox"}
              <Badge count={unread} label="unread" />
            {/if}
          </a>
        {/each}
      </nav>
      <SelectField
        label="Show conversations with"
        labelHidden
        value={agent ?? ALL}
        options={filterOptions}
        onchange={(event) => showList({ agent: event.currentTarget.value || null })}
      />
    </div>

    <div class="mail-rows">
      {#if mail.loadError}
        <Banner tone="error">
          {mail.loadError}
          {#snippet actions()}
            <Button variant="quiet" size="sm" onclick={() => void mail.reload()}>Try again</Button>
          {/snippet}
        </Banner>
      {/if}

      {#if !mail.loaded}
        {#if mail.loadError === null}
          <div class="mail-loading">
            <Skeleton shape="block" height="64px" label="Loading conversations" />
            <Skeleton shape="block" height="64px" />
            <Skeleton shape="block" height="64px" />
            <Skeleton shape="block" height="64px" />
          </div>
        {/if}
      {:else if mail.threads.length === 0}
        <EmptyState>
          {emptyListText(folder, agent)}
          {#snippet actions()}
            {#if folder === "sent"}
              <Button variant="quiet" size="sm" icon="edit" onclick={newMessage}>New message</Button
              >
            {/if}
          {/snippet}
        </EmptyState>
      {:else}
        <ul class="mail-row-list">
          {#each mail.threads as summary (summary.thread_id)}
            <li>
              <MailThreadRow
                {summary}
                {folder}
                {now}
                href={formatLocation(locationAt(threadPlace(summary.thread_id)))}
                selected={summary.thread_id === openId && !composing}
                onopen={(event) => {
                  if (followsInPlace(event)) void openThread(summary.thread_id);
                }}
              />
            </li>
          {/each}
        </ul>
        {#if mail.moreError}
          <p class="mail-problem" role="alert">
            {mail.moreError}
            <Button variant="quiet" size="sm" onclick={(event) => void showOlder(event)}
              >Try again</Button
            >
          </p>
        {:else if mail.next !== null}
          <div class="mail-more">
            <Button
              variant="quiet"
              size="sm"
              loading={mail.loadingMore}
              onclick={(event) => void showOlder(event)}>Show older</Button
            >
          </div>
        {/if}
      {/if}
    </div>
  </section>

  <div class="mail-pane" bind:this={paneElement}>
    <div class="mail-pane-frame">
      {#if place.compose !== null}
        <MailCompose
          to={place.compose.to}
          onsent={(threadId) => void sent(threadId)}
          oncancel={() => void backToList()}
        />
      {:else if missing}
        <section class="mail-missing" aria-label="Conversation not found" tabindex="-1">
          <EmptyState
            variant="block"
            icon="inbox"
            title="That conversation isn't there any more"
            headingLevel={2}
          >
            The link may name a message that was removed. The list shows what's there now.
            {#snippet actions()}
              <Button variant="secondary" icon="back" onclick={() => void backToList()}
                >Back to list</Button
              >
            {/snippet}
          </EmptyState>
        </section>
      {:else if openId !== null}
        <MailThread threadId={openId} onback={() => void backToList()} />
      {:else}
        <div class="mail-pane-idle">
          <EmptyState>Choose a conversation to read it here.</EmptyState>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .mail-header-actions {
    display: flex;
    flex: none;
    align-self: center;
    margin-left: auto;
  }

  /* The list keeps its width and the thread takes the rest, its reading
     column centered in it. As the region narrows (the file panel opening
     beside it, a smaller window), the thread gives up its margins first,
     then shrinks to its floor, and past that the list makes way. */
  .mail {
    display: flex;
    flex: 1;
    min-height: 0;
    container: mail / inline-size;
  }

  .mail-list {
    display: flex;
    flex: none;
    flex-direction: column;
    width: var(--layout-mail-list-width);
    min-width: 0;
    min-height: 0;
    border-right: 1px solid var(--color-line-soft);

    &:focus {
      outline: none;
    }
  }

  .mail-tools {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-10);
    padding: var(--space-14) var(--space-14) var(--space-10);
  }

  .mail-folders {
    display: flex;
    gap: var(--space-2);
    padding: var(--space-2);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
  }

  .mail-folder {
    display: inline-flex;
    flex: 1;
    align-items: center;
    justify-content: center;
    gap: var(--space-6);
    height: 28px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      color: var(--color-text);
    }

    &[aria-current="page"] {
      background: var(--color-stone-3);
      color: var(--color-text);
    }
  }

  .mail-rows {
    flex: 1;
    min-height: 0;
    padding: var(--space-2) var(--space-8) var(--space-24);
    overflow-y: auto;
  }

  .mail-row-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .mail-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding-inline: var(--space-6);
  }

  .mail-more {
    margin-top: var(--space-12);
    padding-inline: var(--space-6);
  }

  .mail-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    margin-top: var(--space-12);
    padding-inline: var(--space-12);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .mail-pane {
    flex: 1;
    min-width: var(--layout-mail-thread-min-width);
    min-height: 0;
    overflow-y: auto;
  }

  .mail-missing:focus {
    outline: none;
  }

  .mail-pane-frame {
    width: 100%;
    max-width: calc(var(--layout-reading-width) + 2 * var(--space-24));
    margin-inline: auto;
    padding: var(--space-20) var(--space-24) var(--space-48);
  }

  .mail-pane-idle {
    display: flex;
    justify-content: center;
    padding-top: var(--space-64);
  }

  /* The split width is --layout-mail-list-width + --layout-mail-thread-min-width:
     narrower than both side by side (a pixel less), one shows at a time. A
     container query can't read the tokens, so the width is written out in
     this file's two queries and nowhere else; mail-layout.test.ts checks them
     against the tokens. */
  @container mail (max-width: 779px) {
    .mail-list {
      flex: 1;
      width: auto;
      border-right: 0;
    }

    .mail-pane {
      min-width: 0;
    }

    .mail[data-open] .mail-list,
    .mail:not([data-open]) .mail-pane {
      display: none;
    }
  }

  /* With the list beside the thread or New message, the list is the way back. */
  @container mail (min-width: 780px) {
    .mail :global(.mail-back) {
      display: none;
    }
  }

  @media (max-width: 760px) {
    .mail-tools {
      padding: var(--space-12) var(--space-16) var(--space-8);
    }

    .mail-rows {
      padding-inline: var(--space-8);
    }

    .mail-folder {
      height: var(--layout-touch-target);
    }

    .mail-pane-frame {
      padding: var(--space-12) var(--space-16) var(--space-40);
    }
  }
</style>
