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
    width?: string;
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
