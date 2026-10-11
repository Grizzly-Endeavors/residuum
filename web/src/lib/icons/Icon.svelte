<script lang="ts">
  import { ICONS, type IconName } from "./icons";

  interface Props {
    name: IconName;
    size?: number;
    class?: string;
  }

  let { name, size = 16, class: className = "" }: Props = $props();
</script>

<!-- Decorative: the control or text beside an icon carries its label. -->
<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  class={className}
  aria-hidden="true"
  xmlns="http://www.w3.org/2000/svg"
>
  {#each ICONS[name] as shape, index (index)}
    {@const paint = shape.solid ? { fill: "currentColor", stroke: "none" } : {}}
    {#if "path" in shape}
      <path d={shape.path} {...paint} />
    {:else if "circle" in shape}
      <circle cx={shape.circle[0]} cy={shape.circle[1]} r={shape.circle[2]} {...paint} />
    {:else}
      <rect
        x={shape.rect[0]}
        y={shape.rect[1]}
        width={shape.rect[2]}
        height={shape.rect[3]}
        rx={shape.rect[4]}
        {...paint}
      />
    {/if}
  {/each}
</svg>

<style>
  svg {
    display: inline-block;
    vertical-align: middle;
    flex-shrink: 0;
  }
</style>
