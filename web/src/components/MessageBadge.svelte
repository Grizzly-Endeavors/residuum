<script lang="ts">
  let {
    content,
    variant,
  }: {
    content: string;
    variant: "system" | "error" | "notice";
  } = $props();
</script>

<div class="msg msg-{variant}">{content}</div>
