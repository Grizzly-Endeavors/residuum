<script lang="ts">
  import type { Snippet } from "svelte";
  import ModalLayer from "./ModalLayer.svelte";

  interface Props {
    open: boolean;
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
