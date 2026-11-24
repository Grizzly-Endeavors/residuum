<script lang="ts" module>
  import type { AppAction } from "../lib/action-registry.svelte";

  export interface ComposerMenu {
    label: string;
    actions: readonly AppAction[];
  }
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { actionRegistry, matchActions } from "../lib/action-registry.svelte";
  import { IconButton, VisuallyHidden } from "../lib/ui";
  import SlashMenu from "./SlashMenu.svelte";

  interface Props {
    value: string;
    label: string;
    placeholder: string;
    menu?: ComposerMenu;
    disabled?: boolean;
    describedby?: string;
    onsend: () => boolean;
    onkeys?: (event: KeyboardEvent, menuShown: boolean) => void;
    onleave?: () => void;
    onpaste?: (event: ClipboardEvent) => void;
    above?: Snippet;
    tools?: Snippet<[Snippet]>;
    sendControl: Snippet;
    children?: Snippet;
  }

  let {
    value = $bindable(),
    label,
    placeholder,
    menu,
    disabled = false,
    describedby,
    onsend,
    onkeys,
    onleave,
    onpaste,
    above,
    tools,
    sendControl,
    children,
  }: Props = $props();

  const MAX_FIELD_HEIGHT = 200;

  const uid = $props.id();
  const labelId = `${uid}-label`;
  const menuId = `${uid}-actions`;

  let field = $state<HTMLTextAreaElement>();

  $effect(() => {
    void value;
    const box = field;
    if (box === undefined) return;
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, MAX_FIELD_HEIGHT))}px`;
  });

  export function focus(): void {
    field?.focus();
  }

  let menuOpen = $state(false);
  let menuFromButton = $state(false);
  let menuQuery = $state("");
  let menuIndex = $state(0);
  const menuActions = $derived(menu === undefined ? [] : matchActions(menu.actions, menuQuery));
  const showMenu = $derived(menuOpen && menuActions.length > 0);

  function openMenu(query: string, fromButton: boolean): void {
    menuOpen = true;
    menuFromButton = fromButton;
    menuQuery = query;
    menuIndex = 0;
  }

  function followTyping(): void {
    if (menu !== undefined && /^\/\S*$/.test(value)) openMenu(value.slice(1), false);
    else menuOpen = false;
  }

  function toggleMenu(): void {
    if (showMenu && menuFromButton) menuOpen = false;
    else openMenu("", true);
    field?.focus();
  }

  function pick(action: AppAction): void {
    if (action.disabled !== undefined) return;
    menuOpen = false;
    if (action.takesText && action.command !== undefined) {
      value = `/${action.command} `;
      field?.focus();
      return;
    }
    value = "";
    void actionRegistry.run(action);
  }

  function menuKey(event: KeyboardEvent): boolean {
    const count = menuActions.length;
    switch (event.key) {
      case "ArrowDown":
        menuIndex = (menuIndex + 1) % count;
        return true;
      case "ArrowUp":
        menuIndex = (menuIndex - 1 + count) % count;
        return true;
      case "Tab": {
        const action = menuActions[menuIndex];
        if (action?.command === undefined) return false;
        value = `/${action.command}${action.takesText === true ? " " : ""}`;
        menuOpen = false;
        return true;
      }
      case "Enter": {
        const action = menuActions[menuIndex];
        if (action !== undefined) pick(action);
        return true;
      }
      case "Escape":
        menuOpen = false;
        return true;
      default:
        return false;
    }
  }

  function send(): void {
    if (onsend()) menuOpen = false;
  }

  // On a touch screen Enter is a new line, as the soft keyboard's key says
  // (`enterkeyhint`), and only the send control sends.
  const touch = new MediaQuery("(pointer: coarse)");

  function onkeydown(event: KeyboardEvent): void {
    if (event.isComposing) return;
    if (showMenu && menuKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !touch.current) {
      event.preventDefault();
      send();
    }
  }

  function onsubmit(event: SubmitEvent): void {
    event.preventDefault();
    send();
    if (event.submitter !== null) field?.focus();
  }

  const watchKeys = (form: HTMLElement): (() => void) => {
    const listener = (event: KeyboardEvent): void => {
      onkeys?.(event, showMenu);
    };
    form.addEventListener("keydown", listener);
    return () => form.removeEventListener("keydown", listener);
  };

  function onfocusout(event: FocusEvent & { currentTarget: HTMLElement }): void {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    menuOpen = false;
    onleave?.();
  }
</script>

{#snippet menuButton()}
  {#if menu !== undefined}
    <IconButton
      icon="slash"
      label={menu.label}
      {disabled}
      aria-expanded={showMenu && menuFromButton}
      aria-controls={showMenu ? menuId : undefined}
      onclick={toggleMenu}
    />
  {/if}
{/snippet}

<form class="composer" aria-labelledby={labelId} {onsubmit} {onfocusout} {@attach watchKeys}>
  {#if showMenu && menu !== undefined}
    <SlashMenu
      id={menuId}
      label={menu.label}
      actions={menuActions}
      active={menuIndex}
      onpick={pick}
      onhover={(index) => (menuIndex = index)}
    />
  {/if}
  {@render above?.()}
  <VisuallyHidden id={labelId}>{label}</VisuallyHidden>
  <div class="composer-field">
    <!-- With a menu, the message box is itself its combobox (ARIA 1.2). -->
    <textarea
      bind:this={field}
      bind:value
      rows="1"
      {placeholder}
      {disabled}
      role={menu === undefined ? undefined : "combobox"}
      aria-labelledby={labelId}
      aria-describedby={describedby}
      aria-autocomplete={menu === undefined ? undefined : "list"}
      aria-expanded={menu === undefined ? undefined : showMenu}
      aria-controls={showMenu ? menuId : undefined}
      aria-activedescendant={showMenu ? `${menuId}-${String(menuIndex)}` : undefined}
      enterkeyhint={touch.current ? "enter" : undefined}
      {onkeydown}
      oninput={followTyping}
      {onpaste}
    ></textarea>
  </div>
  <div class="composer-bar">
    {#if tools}
      {@render tools(menuButton)}
    {:else}
      {@render menuButton()}
    {/if}
    <span class="composer-send">{@render sendControl()}</span>
  </div>
  {@render children?.()}
</form>

<style>
  .composer {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    max-width: var(--layout-reading-width);
    margin: 0 auto;
    padding: var(--space-10) var(--space-10) var(--space-8) var(--space-14);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
    box-shadow: inset 0 0 0 1px var(--color-control-border);
    transition: box-shadow var(--duration-fast) var(--ease-out);

    &:focus-within {
      box-shadow:
        inset 0 0 0 2px var(--color-vein),
        0 0 0 3px var(--color-vein-faint);
    }
  }

  .composer-field textarea {
    display: block;
    width: 100%;
    min-height: 24px;
    max-height: 200px;
    padding: var(--space-2) 0;
    resize: none;
    border: 0;
    background: transparent;
    font-size: var(--font-size-message);
    --leading: var(--line-height-ui);

    &:focus-visible {
      outline: none;
    }

    &:disabled {
      cursor: not-allowed;
    }
  }

  .composer-bar {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    min-width: 0;
  }

  .composer-send {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    margin-left: auto;

    & :global(.ui-icon-button[data-variant="primary"]:disabled) {
      background: var(--color-stone-3);
      color: var(--color-text-3);
      opacity: 1;
    }
  }

  @media (max-width: 760px) {
    .composer {
      padding: var(--space-8) var(--space-8) var(--space-6) var(--space-12);
    }

    .composer-field textarea {
      font-size: var(--font-size-field-phone);
    }
  }
</style>
