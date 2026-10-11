<script lang="ts">
  import type { Snippet } from "svelte";
  import ModalLayer from "./ModalLayer.svelte";

  // A drawer from the left edge: on phones, the rail opens in one. Swiping it
  // left, Esc, the scrim and Back close it.

  interface Props {
    open: boolean;
    /** The drawer's name for assistive technology, such as "Agents and places". */
    label: string;
    initialFocus?: string | false;
    onclose?: () => void;
    children: Snippet;
  }

  let { open = $bindable(false), label, initialFocus, onclose, children }: Props = $props();

  function close(): void {
    open = false;
    onclose?.();
  }
</script>

<ModalLayer {open} frame="left" onclose={close} {label} {initialFocus}>
  {@render children()}
</ModalLayer>
