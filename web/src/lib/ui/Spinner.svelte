<script lang="ts">
  // Decorative: the control or text beside a spinner says what is loading.
  let { size = 12 }: { size?: number } = $props();
</script>

<span class="ui-spinner" style:width="{size}px" style:height="{size}px" aria-hidden="true"></span>

<style>
  .ui-spinner {
    display: inline-block;
    flex: none;
    border: 1.5px solid currentcolor;
    border-right-color: transparent;
    border-radius: 50%;
    animation: ui-spinner-turn var(--duration-spin) var(--ease-linear) infinite;
  }

  @keyframes ui-spinner-turn {
    to {
      transform: rotate(360deg);
    }
  }
</style>
