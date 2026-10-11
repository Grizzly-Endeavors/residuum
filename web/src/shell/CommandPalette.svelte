<script lang="ts">
  import { untrack } from "svelte";
  import {
    actionRegistry,
    groupRuns,
    matchActions,
    type AppAction,
  } from "../lib/action-registry.svelte";
  import { Icon } from "../lib/icons";
  import { IconButton, Kbd, ModalLayer, VisuallyHidden } from "../lib/ui";
  import { PHONE_QUERY } from "../styles/breakpoints";
  import ActionOption from "./ActionOption.svelte";

  // The command palette: everything in the action registry, found by typing.
  // The field keeps focus, the arrow keys move through the results, Enter
  // runs one, and Esc closes. It fills the screen on phones.

  let { open = $bindable(false) }: { open?: boolean } = $props();

  const uid = $props.id();
  const DESCRIPTION = "Search agents, places, sessions, settings and actions";
  let query = $state("");
  let active = $state(0);
  /** A phone's field is too narrow for the whole description. */
  let placeholder = $state(DESCRIPTION);

  const results = $derived(matchActions(actionRegistry.all, query));
  const runs = $derived(groupRuns(results));
  const optionId = (index: number): string => `${uid}-option-${String(index)}`;

  // Each opening starts from an empty search.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      query = "";
      active = 0;
      placeholder = window.matchMedia(PHONE_QUERY).matches ? "Search or jump to" : DESCRIPTION;
    });
  });

  // The results change under the highlight as agents and sessions come and go.
  $effect(() => {
    if (active >= results.length) active = Math.max(results.length - 1, 0);
  });

  $effect(() => {
    if (!open) return;
    document.getElementById(optionId(active))?.scrollIntoView({ block: "nearest" });
  });

  function close(): void {
    open = false;
  }

  function choose(action: AppAction): void {
    if (action.disabled !== undefined) return;
    open = false;
    void actionRegistry.run(action);
  }

  function onkeydown(event: KeyboardEvent): void {
    const count = results.length;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (count > 0) active = (active + (event.key === "ArrowDown" ? 1 : -1) + count) % count;
    } else if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      const action = results[active];
      if (action !== undefined) choose(action);
    }
  }
</script>

<ModalLayer
  {open}
  frame="top"
  width="620px"
  fullscreenOnPhone
  label="Search and commands"
  initialFocus="[data-palette-field]"
  onclose={close}
>
  <div class="palette-search">
    <Icon name="search" size={17} />
    <input
      data-palette-field
      class="palette-field"
      type="text"
      role="combobox"
      aria-expanded="true"
      aria-autocomplete="list"
      aria-controls="{uid}-results"
      aria-activedescendant={results.length > 0 ? optionId(active) : undefined}
      aria-label={DESCRIPTION}
      {placeholder}
      autocomplete="off"
      spellcheck="false"
      bind:value={query}
      oninput={() => (active = 0)}
      {onkeydown}
    />
    <span class="palette-esc"><Kbd keys={["Esc"]} /></span>
    <span class="palette-close">
      <IconButton icon="close" label="Close" size="sm" data-overlay-close onclick={close} />
    </span>
  </div>
  <div
    id="{uid}-results"
    class="palette-results"
    role="listbox"
    aria-label="Results"
    hidden={results.length === 0}
  >
    {#each runs as run, at (`${String(at)}:${run.heading}`)}
      <div role="group" aria-labelledby="{uid}-group-{at}">
        <div id="{uid}-group-{at}" class="palette-heading" role="presentation">{run.heading}</div>
        {#each run.actions as { action, index } (action.id)}
          <ActionOption
            {action}
            id={optionId(index)}
            active={index === active}
            hint={action.hint}
            onpick={() => {
              choose(action);
            }}
            onhover={() => (active = index)}
          />
        {/each}
      </div>
    {/each}
  </div>
  {#if results.length === 0}
    <p class="palette-empty">
      Nothing matches “{query.trim()}”. Try an agent name, a place, or a setting.
    </p>
  {/if}
  <VisuallyHidden>
    <span role="status"
      >{results.length === 1 ? "1 result" : `${String(results.length)} results`}</span
    >
  </VisuallyHidden>
  <footer class="palette-foot" aria-hidden="true">
    <span>↑ ↓ to move</span><span>Enter to run</span><span>Esc to close</span>
  </footer>
</ModalLayer>

<style>
  .palette-search {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-10);
    height: 52px;
    padding: 0 var(--space-14) 0 var(--space-16);
    border-bottom: 1px solid var(--color-line);
    color: var(--color-text-3);
  }

  .palette-field {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--color-text);
    font-size: var(--font-size-message);

    &:focus-visible {
      outline: none;
    }
  }

  .palette-close {
    display: none;
  }

  .palette-results {
    flex: 1 1 auto;
    min-height: 0;
    padding: var(--space-6);
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .palette-heading {
    padding: var(--space-10) var(--space-10) var(--space-4);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .palette-empty {
    padding: var(--space-18);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .palette-foot {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    gap: var(--space-16);
    padding: var(--space-8) var(--space-14);
    border-top: 1px solid var(--color-line);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  @media (max-width: 760px) {
    .palette-search {
      height: 56px;
      padding-right: var(--space-6);
    }

    .palette-field {
      font-size: var(--font-size-field-phone);
    }

    .palette-esc,
    .palette-foot {
      display: none;
    }

    .palette-close {
      display: contents;
    }
  }
</style>
