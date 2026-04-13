<script lang="ts">
  let { content }: { content: string } = $props();
</script>

<div class="msg msg-local-system">
  <pre>{content}</pre>
</div>
