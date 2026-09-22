<script lang="ts">
  let { tone, content }: { tone: "info" | "error"; content: string } = $props();
</script>

<div class="msg msg-status tone-{tone}" role={tone === "error" ? "alert" : "status"}>
  {content}
</div>
