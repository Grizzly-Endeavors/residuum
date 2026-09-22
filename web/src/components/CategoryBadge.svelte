<script lang="ts">
  let { category }: { category: string } = $props();
</script>

<span class="session-badge badge-{category}">{category}</span>
