<script lang="ts">
  import { Button } from "../../lib/ui";

  // The foot of a setup step: Back on the left, the step's forward action on
  // the right.
  interface Props {
    /** Absent on the first step. */
    onBack?: () => void;
    backDisabled?: boolean;
    nextLabel?: string;
    onNext: () => void;
    nextDisabled?: boolean;
    /** The forward action is under way. */
    busy?: boolean;
  }

  let {
    onBack,
    backDisabled = false,
    nextLabel = "Next",
    onNext,
    nextDisabled = false,
    busy = false,
  }: Props = $props();
</script>

<div class="setup-nav">
  {#if onBack}
    <Button disabled={backDisabled} onclick={onBack}>Back</Button>
  {/if}
  <span class="setup-nav-gap"></span>
  <Button variant="primary" disabled={nextDisabled} loading={busy} onclick={onNext}>
    {nextLabel}
  </Button>
</div>

<style>
  .setup-nav {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    margin-top: var(--space-8);
    padding-top: var(--space-16);
    border-top: 1px solid var(--color-line-soft);
  }

  .setup-nav-gap {
    flex: 1;
  }
</style>
