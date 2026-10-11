<script lang="ts">
  import type { Snippet } from "svelte";

  let { children, id }: { children: Snippet; id?: string } = $props();
</script>

<!-- Read by assistive technology, invisible on screen. -->
<span class="ui-visually-hidden" {id}>{@render children()}</span>

<style>
  .ui-visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
    border: 0;
  }
</style>
