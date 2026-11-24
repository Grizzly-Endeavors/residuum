<script module lang="ts">
  import { createContext } from "svelte";

  interface MenuContext {
    chosen: () => void;
  }

  export const [menuContext, setMenuContext] = createContext<MenuContext>();
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import { createAttachmentKey, type Attachment } from "svelte/attachments";
  import FloatingLayer from "./FloatingLayer.svelte";
  import type { FloatAlign, FloatSide, FloatTriggerProps } from "./types";

  interface Props {
    open?: boolean;
    label: string;
    trigger: Snippet<[FloatTriggerProps]>;
    side?: FloatSide;
    align?: FloatAlign;
    heading?: Snippet;
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
  const TYPEAHEAD_PAUSE_MS = 500;
  const ITEMS = '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"]';

  let anchor = $state<HTMLElement | null>(null);
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

  function typeahead(
    event: KeyboardEvent,
    items: HTMLElement[],
    at: number,
  ): HTMLElement | undefined {
    if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return undefined;
    if (event.key === " " && (typed === "" || event.timeStamp - typedAt > TYPEAHEAD_PAUSE_MS)) {
      return undefined;
    }
    typed = event.timeStamp - typedAt > TYPEAHEAD_PAUSE_MS ? event.key : typed + event.key;
    typedAt = event.timeStamp;
    const search = typed.toLowerCase();
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
    if (next === undefined) {
      const focused = items[at];
      if (event.key === " " && focused instanceof HTMLAnchorElement) {
        event.preventDefault();
        focused.click();
      }
      return;
    }
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
