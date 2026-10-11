<script module lang="ts">
  import { createContext } from "svelte";

  interface MenuContext {
    /** An item was chosen: the menu closes and focus goes back to its button. */
    chosen: () => void;
  }

  /** What a MenuItem reads from the Menu around it. */
  export const [menuContext, setMenuContext] = createContext<MenuContext>();
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import { createAttachmentKey, type Attachment } from "svelte/attachments";
  import FloatingLayer from "./FloatingLayer.svelte";
  import type { FloatAlign, FloatSide, FloatTriggerProps } from "./types";

  // A menu button and its menu. The arrow keys, Home and End move through the
  // items, typing jumps to the item that starts with what was typed, Enter
  // and Space choose, and Esc, Tab or a pointer outside close it.

  interface Props {
    open?: boolean;
    /** The menu's accessible name, such as "Manage atlas". */
    label: string;
    /** The menu button. Spread `props` onto it: `<IconButton icon="more" label="More" {...props} />`. */
    trigger: Snippet<[FloatTriggerProps]>;
    side?: FloatSide;
    align?: FloatAlign;
    /** A quiet line above the items, such as the agent and its state. It describes the menu. */
    heading?: Snippet;
    /** MenuItem and MenuSeparator. */
    children: Snippet;
  }

  let {
    open = $bindable(false),
    label,
    trigger,
    side = "bottom",
    align = "start",
    heading,
    children,
  }: Props = $props();

  const uid = $props.id();
  /** Typed letters run together into one search until a pause this long. */
  const TYPEAHEAD_PAUSE_MS = 500;
  const ITEMS = '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"]';

  let anchor = $state<HTMLElement | null>(null);
  /** Where focus starts: an item when opened from the keyboard, the menu itself from a pointer. */
  let startOn: "first" | "last" | "menu" = "first";
  let typed = "";
  let typedAt = 0;

  setMenuContext({
    chosen: () => {
      anchor?.focus();
      open = false;
    },
  });

  const attachAnchor: Attachment<HTMLElement> = (element) => {
    anchor = element;
    return () => {
      if (anchor === element) anchor = null;
    };
  };
  const anchorKey = createAttachmentKey();

  function openFrom(start: typeof startOn): void {
    startOn = start;
    typed = "";
    open = true;
  }

  const triggerProps = $derived<FloatTriggerProps>({
    "aria-haspopup": "menu",
    "aria-expanded": open,
    "aria-controls": open ? uid : undefined,
    // A click from the keyboard or a screen reader reports no pointer presses.
    onclick: (event) => {
      if (open) open = false;
      else openFrom(event.detail === 0 ? "first" : "menu");
    },
    onkeydown: (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        openFrom(event.key === "ArrowDown" ? "first" : "last");
      } else if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (open) open = false;
        else openFrom("first");
      }
    },
    [anchorKey]: attachAnchor,
  });

  function itemsIn(root: HTMLElement): HTMLElement[] {
    return [...root.querySelectorAll<HTMLElement>(ITEMS)];
  }

  function focusStart(layer: HTMLElement): void {
    const menu = layer.querySelector<HTMLElement>('[role="menu"]');
    if (menu === null) return;
    const items = itemsIn(menu);
    const enabled = items.filter((item) => item.getAttribute("aria-disabled") !== "true");
    const target = { first: enabled[0], last: enabled.at(-1), menu }[startOn];
    (target ?? menu).focus();
  }

  /** The item a typed letter leads to, or undefined when the key isn't typing. */
  function typeahead(
    event: KeyboardEvent,
    items: HTMLElement[],
    at: number,
  ): HTMLElement | undefined {
    if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return undefined;
    // Space chooses the item, unless it continues a search.
    if (event.key === " " && (typed === "" || event.timeStamp - typedAt > TYPEAHEAD_PAUSE_MS)) {
      return undefined;
    }
    typed = event.timeStamp - typedAt > TYPEAHEAD_PAUSE_MS ? event.key : typed + event.key;
    typedAt = event.timeStamp;
    const search = typed.toLowerCase();
    // One letter pressed again moves on to the next item that starts with it.
    const first = search.charAt(0);
    const term = search === first.repeat(search.length) ? first : search;
    const from = term.length === 1 ? at + 1 : Math.max(at, 0);
    const order = [...items.slice(from), ...items.slice(0, from)];
    return order.find((item) => item.dataset.label?.toLowerCase().startsWith(term)) ?? items[at];
  }

  function onkeydown(event: KeyboardEvent): void {
    const items = itemsIn(event.currentTarget as HTMLElement);
    const at = items.indexOf(document.activeElement as HTMLElement);
    let next: HTMLElement | undefined;
    if (event.key === "ArrowDown") next = items[(at + 1) % items.length];
    else if (event.key === "ArrowUp") next = items.at(at <= 0 ? -1 : at - 1);
    else if (event.key === "Home") next = items[0];
    else if (event.key === "End") next = items.at(-1);
    else next = typeahead(event, items, at);
    if (next === undefined) return;
    event.preventDefault();
    next.focus();
  }
</script>

{@render trigger(triggerProps)}
<FloatingLayer
  {open}
  {anchor}
  shape="menu"
  {side}
  {align}
  initialFocus={focusStart}
  onclose={() => (open = false)}
>
  {#if heading}
    <div id="{uid}-heading" class="ui-menu-heading">{@render heading()}</div>
  {/if}
  <div
    id={uid}
    role="menu"
    tabindex="-1"
    aria-label={label}
    aria-describedby={heading ? `${uid}-heading` : undefined}
    class="ui-menu"
    {onkeydown}
  >
    {@render children()}
  </div>
</FloatingLayer>

<style>
  .ui-menu {
    display: flex;
    flex-direction: column;

    &:focus-visible {
      outline: none;
    }
  }

  .ui-menu-heading {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    padding: var(--space-6) var(--space-10) var(--space-4);
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }
</style>
