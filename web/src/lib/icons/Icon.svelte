<script lang="ts">
  type IconName = "menu" | "close" | "chevron" | "paperclip" | "reload" | "warning";

  interface Props {
    name: IconName;
    size?: number;
    class?: string;
  }

  let { name, size = 16, class: className = "" }: Props = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  class={className}
  aria-hidden="true"
  xmlns="http://www.w3.org/2000/svg"
>
  <g stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
    {#if name === "menu"}
      <path d="M3 4.75h10M3 8h10M3 11.25h10" />
    {:else if name === "close"}
      <path d="M4 4l8 8M12 4l-8 8" />
    {:else if name === "chevron"}
      <path d="M4 6l4 4 4-4" />
    {:else if name === "paperclip"}
      <path
        d="M13.5 7.5l-5.793 5.793a3.5 3.5 0 01-4.95-4.95L9.05 2.05a2.25 2.25 0 013.182 3.182L5.94 11.525a1 1 0 01-1.414-1.414L10.818 3.818"
      />
    {:else if name === "reload"}
      <path d="M13 3v4h-4" />
      <path d="M13 7a5 5 0 10-2 3.5" />
    {:else if name === "warning"}
      <path d="M8 2.5L14 12.5L2 12.5Z" />
      <path d="M8 6v3" />
      <path d="M8 10.6v0.01" />
    {/if}
  </g>
</svg>

<style>
  svg {
    display: inline-block;
    vertical-align: middle;
    flex-shrink: 0;
  }
</style>
