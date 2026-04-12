<script lang="ts">
  import { onDestroy } from "svelte";

  interface Props {
    onConfirm: () => void;
    label?: string;
    armedLabel?: string;
    title?: string;
    class?: string;
    disabled?: boolean;
  }

  let {
    onConfirm,
    label = "Remove",
    armedLabel = "Remove?",
    title = "",
    class: className = "btn btn-sm btn-danger",
    disabled = false,
  }: Props = $props();

  let armed = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  onDestroy(() => {
    if (timer) clearTimeout(timer);
  });

  function handleClick() {
    if (armed) {
      armed = false;
      if (timer) clearTimeout(timer);
      onConfirm();
      return;
    }
    armed = true;
    timer = setTimeout(() => {
      armed = false;
    }, 2000);
  }

  function handleBlur() {
    if (armed) {
      armed = false;
      if (timer) clearTimeout(timer);
    }
  }
</script>

<button
  type="button"
  class={className}
  class:armed
  onclick={handleClick}
  onblur={handleBlur}
  {disabled}
  {title}
>
  {armed ? armedLabel : label}
</button>
