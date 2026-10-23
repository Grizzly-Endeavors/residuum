<script lang="ts">
  import { uploadLimitHint } from "./thread-model";

  // The quiet line in a composer's foot, shared by the reply and New message:
  // what a file may weigh, and when the recipients will see the mail. While a
  // send waits on uploads it leads with that, in a live region that is always
  // there, so the change is spoken as well as seen.

  interface Props {
    /** The send is waiting for files still uploading. */
    waiting: boolean;
    /** Who sees it and when, or what is missing before it can go. */
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
