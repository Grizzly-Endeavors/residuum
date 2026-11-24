<script lang="ts">
  import { uploadLimitHint } from "./thread-model";

  interface Props {
    waiting: boolean;
    delivery: string;
  }

  let { waiting, delivery }: Props = $props();
</script>

<span class="composer-hint">
  <span aria-live="polite">{waiting ? "Sending once the files finish uploading." : ""}</span>
  {uploadLimitHint()}
  {delivery}
</span>

<style>
  .composer-hint {
    flex: 1;
    min-width: 120px;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
