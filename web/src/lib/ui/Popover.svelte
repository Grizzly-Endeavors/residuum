<script lang="ts">
  import type { Snippet } from "svelte";
  import { createAttachmentKey, type Attachment } from "svelte/attachments";
  import FloatingLayer from "./FloatingLayer.svelte";
  import type { FloatAlign, FloatSide, FloatTriggerProps } from "./types";

  // A button and the small panel of controls it opens beside itself. The
  // page stays live behind it: Esc, Tab past either end, or a pointer outside
  // closes it, and focus goes back to the button.

  interface Props {
    open?: boolean;
    /** The panel's accessible name, such as "Model for atlas". */
    label: string;
    /** The button. Spread `props` onto it: `<Button {...props}>claude-9</Button>`. */
    trigger: Snippet<[FloatTriggerProps]>;
    side?: FloatSide;
    align?: FloatAlign;
    /** The panel's width; 300px by default. */
    width?: string;
    /** What takes focus on open: `[data-autofocus]` by default, else the first control. */
    initialFocus?: string;
    children: Snippet;
  }

  let {
    open = $bindable(false),
    label,
    trigger,
    side = "bottom",
    align = "start",
    width,
    initialFocus,
    children,
  }: Props = $props();

  const uid = $props.id();
  let anchor = $state<HTMLElement | null>(null);

  const attachAnchor: Attachment<HTMLElement> = (element) => {
    anchor = element;
    return () => {
      if (anchor === element) anchor = null;
    };
  };
  const anchorKey = createAttachmentKey();

  const triggerProps = $derived<FloatTriggerProps>({
    "aria-haspopup": "dialog",
    "aria-expanded": open,
    "aria-controls": open ? uid : undefined,
    onclick: () => {
      open = !open;
    },
    [anchorKey]: attachAnchor,
  });
</script>

{@render trigger(triggerProps)}
<FloatingLayer
  {open}
  {anchor}
  shape="popover"
  {side}
  {align}
  {width}
  {initialFocus}
  id={uid}
  role="dialog"
  aria-label={label}
  onclose={() => (open = false)}
>
  {@render children()}
</FloatingLayer>
