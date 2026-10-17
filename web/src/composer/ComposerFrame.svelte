<script lang="ts" module>
  import type { AppAction } from "../lib/action-registry.svelte";

  /** The actions under `/`, and the name the menu and its button go by. */
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

  // What the chat's composer and a session's message box share: the box with
  // its boundary and focus ring, a message box that grows with its text,
  // Enter to send (a new line on a touch screen, where only the send control
  // sends), and, when there are actions to offer, the `/` menu with the
  // message box as its combobox. What sits above the message box, the tools
  // in the bar and the send control are the owner's.

  interface Props {
    value: string;
    /** The message box's name, such as "Message atlas". */
    label: string;
    placeholder: string;
    /** The `/` menu; without one the message box is a plain text box. */
    menu?: ComposerMenu;
    disabled?: boolean;
    /** The id of what says why the message box is disabled. */
    describedby?: string;
    /** Send what is typed. False when it couldn't go: the menu stays as it was. */
    onsend: () => boolean;
    /**
     * Keys from anywhere in the composer, heard before the message box's own
     * handler, so `menuShown` says whether the open menu is about to claim one.
     */
    onkeys?: (event: KeyboardEvent, menuShown: boolean) => void;
    /** Focus left the composer. */
    onleave?: () => void;
    onpaste?: (event: ClipboardEvent) => void;
    /** Lines above the message box, such as attached images. */
    above?: Snippet;
    /** The bar's tools, given the `/` menu's button to place among them. */
    tools?: Snippet<[Snippet]>;
    /** The send control, at the end of the bar. */
    sendControl: Snippet;
    /** Anything else the form holds, such as a hidden file input. */
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

  /** The tallest the message box grows before it scrolls. */
  const MAX_FIELD_HEIGHT = 200;

  const uid = $props.id();
  const labelId = `${uid}-label`;
  const menuId = `${uid}-actions`;

  let field = $state<HTMLTextAreaElement>();

  // The message box grows with its text, up to a limit, however the text changed.
  $effect(() => {
    void value;
    const box = field;
    if (box === undefined) return;
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, MAX_FIELD_HEIGHT))}px`;
  });

  /** Put the keyboard in the message box. */
  export function focus(): void {
    field?.focus();
  }

  // ── The `/` menu ───────────────────────────────────────────────────

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

  // `/` as the first character, with no space yet, narrows the menu by what follows.
  function followTyping(): void {
    if (menu !== undefined && /^\/\S*$/.test(value)) openMenu(value.slice(1), false);
    else menuOpen = false;
  }

  function toggleMenu(): void {
    if (showMenu && menuFromButton) menuOpen = false;
    else openMenu("", true);
    field?.focus();
  }

  // A disabled action stays in the menu with its reason, and does nothing.
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
        // Filled in, to read over or add to before sending.
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
        // Claimed here, so Esc closes the menu rather than doing what the composer's owner does with it.
        menuOpen = false;
        return true;
      default:
        return false;
    }
  }

  // ── Sending ────────────────────────────────────────────────────────

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

  // The send control is about to be disabled or change, either of which
  // would drop focus to the page: put it back in the box first.
  function onsubmit(event: SubmitEvent): void {
    event.preventDefault();
    send();
    if (event.submitter !== null) field?.focus();
  }

  // This listener is on the form itself, so it hears a key before the message
  // box's delegated handler does.
  const watchKeys = (form: HTMLElement): (() => void) => {
    const listener = (event: KeyboardEvent): void => {
      onkeys?.(event, showMenu);
    };
    form.addEventListener("keydown", listener);
    return () => form.removeEventListener("keydown", listener);
  };

  // A press anywhere else closes the menu.
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
    /* A text field's boundary and focus ring (see Input): the control border at rest, vein at double weight with a halo while focused. */
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

    /* The composer's own ring shows focus. */
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

  /* Never narrower than its button: a long model name gives way first. */
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
