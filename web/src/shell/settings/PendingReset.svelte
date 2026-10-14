<script lang="ts">
  import type { PendingResetInfo } from "../../lib/generated/PendingResetInfo";
  import { cancelPendingReset } from "../../lib/remote-access-api";
  import { toast } from "../../lib/toast.svelte";
  import { Banner, Button } from "../../lib/ui";
  import type { RemoteAct } from "./remote-access-act";

  // A reset by email waiting to take effect. When this instance asked for it,
  // the line says what happens next; when another instance did, it is a
  // warning, with a Cancel for an instance that is already set up, because a
  // reset replaces every other instance's certificate account. What the other
  // instance says about itself is plain text.

  interface Props {
    reset: PendingResetInfo | null;
    busy: string | null;
    act: RemoteAct;
  }

  let { reset, busy, act }: Props = $props();

  const effectiveAt = $derived(
    reset?.effective_at ? new Date(reset.effective_at).toLocaleString() : null,
  );

  function cancel(): Promise<void> {
    return act("cancel-reset", "Couldn't cancel the reset.", async () => {
      await cancelPendingReset();
      toast.success("The reset was cancelled. Your address stays as it is.");
    });
  }
</script>

{#if reset !== null}
  {#if reset.own}
    <Banner title="Reset by email">
      {#if reset.confirmed && effectiveAt !== null}
        Confirmed. This instance takes over your address at {effectiveAt}, unless an instance that
        is already set up cancels it. The new recovery code shows here then.
      {:else}
        Check your email for the reset link and confirm it there. Until you do, nothing changes, and
        your earlier recovery code still works.
      {/if}
    </Banner>
  {:else}
    <Banner tone="warn" title="Another instance is trying to take over your address">
      The instance "{reset.slug}" asked for a reset by email.
      {#if reset.confirmed && effectiveAt !== null}
        It was confirmed and takes effect at {effectiveAt}.
      {:else}
        It hasn't been confirmed yet.
      {/if}
      If it goes through, its certificate account replaces every other one.
      {#if reset.cancellable}
        If this wasn't you, cancel it.
      {:else}
        If this wasn't you, use the cancel link in the email, or cancel it from an instance that is
        already set up.
      {/if}
      {#snippet actions()}
        {#if reset.cancellable}
          <Button
            size="sm"
            loading={busy === "cancel-reset"}
            disabled={busy !== null && busy !== "cancel-reset"}
            onclick={() => void cancel()}
          >
            Cancel the reset
          </Button>
        {/if}
      {/snippet}
    </Banner>
  {/if}
{/if}
