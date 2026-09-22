<script lang="ts">
  import { tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { Icon } from "../lib/icons";
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

  let finishedOpen = $state(false);
  let headingEl: HTMLHeadingElement | undefined = $state();

  const sessions = ws.sessions;
  let selectedRunId = $derived(sessions.view?.runId ?? null);

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
      {#if sessions.live.length > 0}
        <ul class="sessions-list" aria-label="Live sessions">
          {#each sessions.live as session (session.run_id)}
            <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
          {/each}
        </ul>
      {:else}
        <p class="sessions-empty">
          Nothing is running. Work your agent hands off, scheduled pulses, and conversations with
          other people show up here while they run.
        </p>
      {/if}

      <div class="sessions-finished">
        <button
          type="button"
          class="sessions-disclosure"
          aria-expanded={finishedOpen}
          aria-controls="sessions-finished-list"
          onclick={() => (finishedOpen = !finishedOpen)}
        >
          <span class="sessions-disclosure-chevron" class:open={finishedOpen}>
            <Icon name="chevron" size={12} />
          </span>
          Finished
          {#if sessions.completed.length > 0}
            <span class="sessions-disclosure-count">
              {sessions.completed.length}{sessions.nextCursor ? "+" : ""}
            </span>
          {/if}
        </button>
        {#if finishedOpen}
          <div id="sessions-finished-list">
            {#if sessions.completed.length > 0}
              <ul class="sessions-list" aria-label="Finished sessions">
                {#each sessions.completed as session (session.run_id)}
                  <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
                {/each}
              </ul>
            {:else}
              <p class="sessions-empty">Nothing has finished yet.</p>
            {/if}
            {#if sessions.nextCursor}
              <button
                type="button"
                class="sessions-text-btn sessions-more"
                disabled={sessions.loadingMore}
                onclick={() => void sessions.loadMore()}
              >
                {sessions.loadingMore ? "Loading…" : "Show older"}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</aside>
