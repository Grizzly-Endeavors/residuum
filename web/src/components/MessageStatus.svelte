<script lang="ts">
  let { tone, content, details }: { tone: "info" | "error"; content: string; details?: string } =
    $props();
</script>

<div class="msg msg-status tone-{tone}" role={tone === "error" ? "alert" : "status"}>
  {#if details}
    <details class="msg-status-details">
      <summary>{content}</summary>
      <pre class="msg-status-detail-body">{details}</pre>
    </details>
  {:else}
    {content}
  {/if}
</div>
