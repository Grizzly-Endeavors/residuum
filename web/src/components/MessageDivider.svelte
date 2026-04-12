<script lang="ts">
  let { label, variant = "day" }: { label: string; variant?: "episode" | "day" } = $props();
</script>

<div class="msg msg-divider variant-{variant}">
  <span class="msg-divider-line"></span>
  <span class="msg-divider-label">{label}</span>
  <span class="msg-divider-line"></span>
</div>
