<script lang="ts">
  import { tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { Icon } from "../lib/icons";
  import {
    SESSION_CATEGORIES,
    categoryDescription,
    categoryHeading,
    categoryIdleText,
    groupByCategory,
  } from "../lib/session-format";
  import type { SessionCategory } from "../lib/types";
  import SessionRow from "./SessionRow.svelte";

  let {
    overlay,
    onClose,
    onSelect,
  }: {
    /** Shown as a drawer over the page (narrow screens) rather than a column. */
    overlay: boolean;
    onClose: () => void;
    onSelect: (runId: string) => void;
  } = $props();

  /** Groups the user has collapsed; every group starts open. */
  let collapsed = $state<Record<SessionCategory, boolean>>({
    external: false,
    scheduled: false,
    spawned: false,
    artifact: false,
  });
  /** Groups whose finished runs are shown; every group starts closed. */
  let finishedOpen = $state<Record<SessionCategory, boolean>>({
    external: false,
    scheduled: false,
    spawned: false,
    artifact: false,
  });
  let headingEl: HTMLHeadingElement | undefined = $state();

  const sessions = ws.sessions;
  let selectedRunId = $derived(sessions.view?.runId ?? null);

  let liveByCategory = $derived(groupByCategory(sessions.live));

  let sidebarEl: HTMLElement | undefined = $state();

  // As a drawer, take focus when opened so keyboard users land inside it.
  $effect(() => {
    if (!overlay) return;
    void tick().then(() => headingEl?.focus());
  });

  const FOCUSABLE =
    'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

  // A list update can remove the focused row out from under a keyboard
  // user, and focus falls to <body>, outside the modal drawer. Put it on the
  // row now in that spot (or the one before, at the end), else the heading.
  $effect(() => {
    const el = sidebarEl;
    if (!overlay || !el) return;
    let focused: { el: HTMLElement; list: HTMLElement | null; index: number } | null = null;
    const rowButtons = (list: HTMLElement | null): HTMLButtonElement[] =>
      list?.isConnected ? Array.from(list.querySelectorAll("button")) : [];
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof HTMLElement)) return;
      const list = target.closest<HTMLElement>(".sessions-list");
      focused = { el: target, list, index: rowButtons(list).findIndex((b) => b === target) };
    };
    // Focus leaving an element that's still there was the user's doing;
    // only a removal needs rescuing.
    const onFocusOut = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof HTMLElement)) return;
      // Chrome fires this while a focused element is being removed, so judge
      // once the removal is done.
      queueMicrotask(() => {
        if (target.isConnected && focused?.el === target) focused = null;
      });
    };
    const observer = new MutationObserver(() => {
      if (!focused || (document.activeElement && document.activeElement !== document.body)) {
        return;
      }
      const rows = rowButtons(focused.list);
      const next = focused.index >= 0 ? rows[Math.min(focused.index, rows.length - 1)] : undefined;
      (next ?? headingEl)?.focus();
    });
    el.addEventListener("focusin", onFocusIn);
    el.addEventListener("focusout", onFocusOut);
    observer.observe(el, { childList: true, subtree: true });
    return () => {
      el.removeEventListener("focusin", onFocusIn);
      el.removeEventListener("focusout", onFocusOut);
      observer.disconnect();
    };
  });

  // As a modal drawer, Tab and Shift+Tab cycle within it.
  function trapFocus(event: KeyboardEvent) {
    if (!overlay || event.key !== "Tab" || !sidebarEl) return;
    const focusable = Array.from(sidebarEl.querySelectorAll<HTMLElement>(FOCUSABLE));
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    const active = document.activeElement;
    const inside = active instanceof Node && sidebarEl.contains(active);
    if (event.shiftKey && (active === first || !inside || active === headingEl)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !inside)) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<aside
  id="sessions-sidebar"
  class="sessions-sidebar"
  class:overlay
  role={overlay ? "dialog" : undefined}
  aria-modal={overlay ? "true" : undefined}
  aria-labelledby="sessions-sidebar-title"
  bind:this={sidebarEl}
  onkeydown={trapFocus}
>
  <div class="sessions-head">
    <h2 id="sessions-sidebar-title" class="sessions-title" tabindex="-1" bind:this={headingEl}>
      Sessions
    </h2>
    {#if sessions.live.length > 0}
      <span class="sessions-live-count">{sessions.live.length} live</span>
    {/if}
    <button
      type="button"
      class="sessions-close"
      onclick={onClose}
      aria-label="Hide sessions"
      title="Hide sessions"
    >
      <Icon name="close" size={14} />
    </button>
  </div>

  <div class="sessions-scroll">
    {#if sessions.listError}
      <div class="sessions-error" role="alert">
        <p>{sessions.listError}</p>
        <button type="button" class="sessions-text-btn" onclick={() => void sessions.refresh()}>
          Try again
        </button>
      </div>
    {/if}

    {#if !sessions.loaded && !sessions.listError}
      <p class="sessions-empty">Loading sessions…</p>
    {:else if sessions.loaded}
      {#each SESSION_CATEGORIES as category (category)}
        {@const live = liveByCategory[category]}
        {@const finished = sessions.completed[category]}
        <section class="sessions-group" aria-labelledby="sessions-group-{category}-heading">
          <h3 class="sessions-group-heading" id="sessions-group-{category}-heading">
            <button
              type="button"
              class="sessions-disclosure sessions-group-toggle"
              aria-expanded={!collapsed[category]}
              aria-controls="sessions-group-{category}"
              title={categoryDescription(category)}
              onclick={() => (collapsed[category] = !collapsed[category])}
            >
              <span class="sessions-disclosure-chevron" class:open={!collapsed[category]}>
                <Icon name="chevron" size={12} />
              </span>
              {categoryHeading(category)}
              {#if live.length > 0}
                <span class="sessions-group-live-count">{live.length} live</span>
              {/if}
            </button>
          </h3>
          {#if !collapsed[category]}
            <div id="sessions-group-{category}" class="sessions-group-body">
              {#if live.length > 0}
                <ul class="sessions-list" aria-label="Live {category} sessions">
                  {#each live as session (session.run_id)}
                    <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
                  {/each}
                </ul>
              {:else}
                <p class="sessions-empty">{categoryIdleText(category)}</p>
              {/if}

              <div class="sessions-finished">
                <button
                  type="button"
                  class="sessions-disclosure sessions-finished-toggle"
                  aria-expanded={finishedOpen[category]}
                  aria-controls="sessions-finished-{category}"
                  onclick={() => (finishedOpen[category] = !finishedOpen[category])}
                >
                  <span class="sessions-disclosure-chevron" class:open={finishedOpen[category]}>
                    <Icon name="chevron" size={12} />
                  </span>
                  Finished
                  {#if finished.runs.length > 0}
                    <span class="sessions-disclosure-count">
                      {finished.runs.length}{finished.nextCursor ? "+" : ""}
                    </span>
                  {/if}
                </button>
                {#if finishedOpen[category]}
                  <div id="sessions-finished-{category}">
                    {#if finished.runs.length > 0}
                      <ul class="sessions-list" aria-label="Finished {category} sessions">
                        {#each finished.runs as session (session.run_id)}
                          <SessionRow
                            {session}
                            selected={session.run_id === selectedRunId}
                            {onSelect}
                          />
                        {/each}
                      </ul>
                    {:else}
                      <p class="sessions-empty">Nothing has finished yet.</p>
                    {/if}
                    {#if finished.nextCursor}
                      <button
                        type="button"
                        class="sessions-text-btn sessions-more"
                        disabled={finished.loadingMore}
                        onclick={() => void finished.loadMore()}
                      >
                        {finished.loadingMore ? "Loading…" : "Show older"}
                      </button>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        </section>
      {/each}
    {/if}
  </div>
</aside>
