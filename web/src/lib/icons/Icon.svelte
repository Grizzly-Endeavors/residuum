<script lang="ts">
  type IconName =
    | "menu"
    | "close"
    | "chevron"
    | "paperclip"
    | "reload"
    | "warning"
    | "pulse"
    | "bug"
    | "spark"
    | "copy"
    | "check"
    | "inbox"
    | "stop"
    | "sessions"
    | "back"
    | "expand"
    | "collapse"
    | "restore";

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
    {:else if name === "expand"}
      <path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10" />
    {:else if name === "collapse"}
      <path d="M6 2.5V6H2.5M10 2.5V6h3.5M10 13.5V10h3.5M6 13.5V10H2.5" />
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
    {:else if name === "pulse"}
      <path d="M3 9.5h2.5" />
      <path d="M6.75 6.5h2.5" />
      <path d="M10.5 9.5h2.5" />
    {:else if name === "bug"}
      <!-- antennae -->
      <path d="M5.5 3.5l1.25 1.75M10.5 3.5l-1.25 1.75" />
      <!-- body shell -->
      <ellipse cx="8" cy="9" rx="3.25" ry="4.25" />
      <!-- spine seam -->
      <path d="M8 5.25v7.5" />
      <!-- legs -->
      <path d="M4.75 7.5l-2 -1M11.25 7.5l2 -1" />
      <path d="M4.75 9.5h-2.25M11.25 9.5h2.25" />
      <path d="M4.75 11.5l-2 1.25M11.25 11.5l2 1.25" />
    {:else if name === "spark"}
      <!-- four-pointed star (used by the integrated feedback button) -->
      <path d="M8 2v4M8 10v4M2 8h4M10 8h4" />
      <path d="M4.5 4.5l1.75 1.75M11.5 11.5l-1.75 -1.75" opacity="0.5" />
      <path d="M11.5 4.5l-1.75 1.75M4.5 11.5l1.75 -1.75" opacity="0.5" />
    {:else if name === "copy"}
      <rect x="5" y="5" width="8" height="9" rx="1" />
      <path d="M5 11H3.5a1 1 0 01-1-1V3.5a1 1 0 011-1H10a1 1 0 011 1V5" />
    {:else if name === "check"}
      <path d="M3 8.5l3 3 7-7" />
    {:else if name === "inbox"}
      <rect x="2" y="4" width="12" height="8" rx="1" />
      <path d="M2 5l6 4 6-4" />
    {:else if name === "sessions"}
      <!-- strata: offset layers, one per running agent -->
      <path d="M2.5 4h7M5 8h8.5M3.5 12h7" />
      <circle cx="12" cy="4" r="1" />
      <circle cx="2.5" cy="8" r="1" />
      <circle cx="12.5" cy="12" r="1" />
    {:else if name === "back"}
      <path d="M13 8H3.5M7 4.5L3.5 8 7 11.5" />
    {:else if name === "restore"}
      <!-- a box (the archive) with an item lifting back out of it -->
      <rect x="2.5" y="9" width="11" height="4.5" rx="1" />
      <path d="M8 9.5V3M8 3L5.25 5.75M8 3l2.75 2.75" />
    {/if}
  </g>
  {#if name === "stop"}
    <rect x="4.5" y="4.5" width="7" height="7" rx="1" fill="currentColor" />
  {/if}
</svg>

<style>
  svg {
    display: inline-block;
    vertical-align: middle;
    flex-shrink: 0;
  }
</style>
