<script lang="ts">
  interface Props {
    size?: number;
  }
  let { size = 26 }: Props = $props();
</script>

<svg
  class="brand-mark"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  xmlns="http://www.w3.org/2000/svg"
  aria-hidden="true"
>
  <defs>
    <linearGradient id="residuum-vein-grad" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" class="vein-stop-edge" />
      <stop offset="50%" class="vein-stop-core" />
      <stop offset="100%" class="vein-stop-edge" />
    </linearGradient>
  </defs>
  <path
    class="brand-mark-outline"
    d="M12 2 L22 12 L12 22 L2 12 Z"
    fill="none"
    stroke-width="1.3"
    stroke-linejoin="round"
  />
  <path
    class="brand-mark-vein"
    d="M7 7 L17 17"
    stroke="url(#residuum-vein-grad)"
    stroke-width="1.1"
    stroke-linecap="round"
  />
</svg>

<style>
  .brand-mark {
    display: block;
    flex-shrink: 0;
    filter: drop-shadow(0 0 6px rgba(59, 139, 219, 0.22));
  }

  .brand-mark-outline {
    stroke: var(--vein-dim);
  }

  .vein-stop-edge {
    stop-color: var(--vein-bright);
    stop-opacity: 0;
  }

  .vein-stop-core {
    stop-color: var(--vein-bright);
    stop-opacity: 0.95;
  }
</style>
