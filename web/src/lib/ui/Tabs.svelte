<script lang="ts" generics="T extends string">
  import type { Snippet } from "svelte";
  import Badge from "./Badge.svelte";
  import type { TabItem } from "./types";

  // A tab list and the one panel it controls. Arrow keys move between tabs
  // and select the one they land on; Home and End jump to the ends.
  interface Props {
    /** Names the tab list for assistive technology. */
    label: string;
    tabs: readonly TabItem<T>[];
    selected: T;
    onchange?: (selected: T) => void;
    /** The panel's content for the selected tab. */
    children: Snippet<[T]>;
  }

  let { label, tabs, selected = $bindable(), onchange, children }: Props = $props();

  const uid = $props.id();
  const panelId = `${uid}-panel`;
  const tabId = (value: T): string => `${uid}-tab-${value}`;

  function select(tab: TabItem<T>): void {
    if (tab.disabled || tab.value === selected) return;
    selected = tab.value;
    onchange?.(tab.value);
  }

  function onkeydown(event: KeyboardEvent): void {
    const enabled = tabs.filter((tab) => !tab.disabled);
    const current = enabled.findIndex((tab) => tab.value === selected);
    let index: number;
    switch (event.key) {
      case "ArrowRight":
        index = (current + 1) % enabled.length;
        break;
      case "ArrowLeft":
        index = (current - 1 + enabled.length) % enabled.length;
        break;
      case "Home":
        index = 0;
        break;
      case "End":
        index = enabled.length - 1;
        break;
      default:
        return;
    }
    const target = enabled[index];
    if (target === undefined) return;
    event.preventDefault();
    select(target);
    document.getElementById(tabId(target.value))?.focus();
  }
</script>

<div class="ui-tabs">
  <div class="ui-tab-list" role="tablist" aria-label={label}>
    {#each tabs as tab (tab.value)}
      <button
        type="button"
        role="tab"
        id={tabId(tab.value)}
        class="ui-tab"
        aria-selected={tab.value === selected}
        aria-controls={panelId}
        tabindex={tab.value === selected ? 0 : -1}
        disabled={tab.disabled}
        {onkeydown}
        onclick={() => {
          select(tab);
        }}
      >
        {tab.label}
        {#if tab.count !== undefined}
          <Badge count={tab.count} />
        {/if}
      </button>
    {/each}
  </div>
  <div
    id={panelId}
    class="ui-tab-panel"
    role="tabpanel"
    aria-labelledby={tabId(selected)}
    tabindex="0"
  >
    {@render children(selected)}
  </div>
</div>

<style>
  .ui-tab-list {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4);
  }

  .ui-tab {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    height: 30px;
    padding: 0 var(--space-12);
    border-radius: var(--corner-pill);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);
  }

  .ui-tab:not(:disabled):hover {
    color: var(--color-text);
  }

  .ui-tab[aria-selected="true"] {
    background: var(--color-stone-3);
    color: var(--color-text);
  }

  .ui-tab:disabled {
    opacity: 0.4;
  }

  .ui-tab-panel {
    padding-top: var(--space-14);
    border-radius: var(--corner-sm);
  }

  @media (max-width: 760px) {
    .ui-tab {
      height: var(--layout-touch-target);
      padding: 0 var(--space-16);
    }
  }
</style>
