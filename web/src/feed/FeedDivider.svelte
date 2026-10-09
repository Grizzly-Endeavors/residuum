<script lang="ts">
  // A day's start, or above a loaded episode its id and date.

  let { label }: { label: string } = $props();
</script>

<div class="feed-divider" role="separator" aria-label={label}>
  {label}
</div>

<style>
  .feed-divider {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    margin: var(--space-4) 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    &::before,
    &::after {
      content: "";
      flex: 1;
      height: 1px;
      background: var(--color-line-soft);
    }
  }
</style>
