<script lang="ts">
  import { Icon, type IconName } from "../icons";
  import { menuContext } from "./Menu.svelte";

  interface Props {
    label: string;
    icon?: IconName;
    hint?: string;
    disabled?: boolean;
    tone?: "default" | "danger";
    checked?: boolean;
    href?: string;
    onselect?: () => void;
  }

  let {
    label,
    icon,
    hint,
    disabled = false,
    tone = "default",
    checked,
    href,
    onselect,
  }: Props = $props();

  const menu = menuContext();
  const checkbox = $derived(checked !== undefined);

  function choose(event: MouseEvent): void {
    if (disabled) {
      event.preventDefault();
      return;
    }
    if (!checkbox) menu.chosen();
    onselect?.();
  }

  function focusOnPointer(event: PointerEvent): void {
    const item = event.currentTarget as HTMLElement;
    if (document.activeElement !== item) item.focus({ preventScroll: true });
  }

  function leaveToMenu(event: PointerEvent): void {
    const item = event.currentTarget as HTMLElement;
    if (document.activeElement !== item) return;
    item.closest<HTMLElement>('[role="menu"]')?.focus({ preventScroll: true });
  }
</script>

{#snippet content()}
  {#if icon}
    <span class="ui-menu-item-icon"><Icon name={icon} size={15} /></span>
  {/if}
  <span class="ui-menu-item-label">{label}</span>
  {#if hint}
    <span class="ui-menu-item-hint">{hint}</span>
  {/if}
  {#if checkbox}
    <span class="ui-menu-item-check"><Icon name="check" size={15} /></span>
  {/if}
{/snippet}

<!-- Pointer and keyboard share one highlight: the focused item. -->
{#if href !== undefined}
  <a
    role="menuitem"
    {href}
    target="_blank"
    rel="noopener noreferrer"
    aria-disabled={disabled || undefined}
    tabindex="-1"
    class="ui-menu-item"
    data-tone={tone}
    data-label={label}
    onclick={choose}
    onpointermove={focusOnPointer}
    onpointerleave={leaveToMenu}
  >
    {@render content()}
  </a>
{:else}
  <button
    type="button"
    role={checkbox ? "menuitemcheckbox" : "menuitem"}
    aria-checked={checkbox ? checked : undefined}
    aria-disabled={disabled || undefined}
    tabindex="-1"
    class="ui-menu-item"
    data-tone={tone}
    data-label={label}
    onclick={choose}
    onpointermove={focusOnPointer}
    onpointerleave={leaveToMenu}
  >
    {@render content()}
  </button>
{/if}

<style>
  .ui-menu-item {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    width: 100%;
    min-height: 32px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    text-decoration: none;
    white-space: nowrap;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:focus {
      outline: none;
      background: var(--color-stone-4);
      color: var(--color-text);
    }

    &:focus-visible {
      outline: var(--focus-outline-width) solid var(--color-vein-bright);
      outline-offset: calc(-1 * var(--focus-outline-width));
    }

    &:active:not([aria-disabled="true"]) {
      transform: translateY(1px);
    }
  }

  .ui-menu-item[data-tone="danger"] {
    color: var(--color-err-text);

    &:focus {
      background: var(--color-err-tint);
      color: var(--color-text);
    }
  }

  .ui-menu-item[aria-disabled="true"] {
    cursor: default;

    & > * {
      opacity: 0.4;
    }
  }

  .ui-menu-item-icon {
    display: grid;
    flex: none;
    place-items: center;
  }

  .ui-menu-item-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .ui-menu-item-hint {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);

    .ui-menu-item:focus & {
      color: var(--color-text-2);
    }
  }

  .ui-menu-item-check {
    display: grid;
    flex: none;
    place-items: center;
    color: var(--color-vein-bright);
    visibility: hidden;

    .ui-menu-item[aria-checked="true"] & {
      visibility: visible;
    }
  }

  @media (max-width: 760px) {
    .ui-menu-item {
      min-height: var(--layout-touch-target);
    }
  }
</style>
