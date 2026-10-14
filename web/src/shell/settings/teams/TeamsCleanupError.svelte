<script lang="ts">
  import { Banner, Disclosure } from "../../../lib/ui";
  import type { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();
</script>

{#if state.cleanupError !== null}
  <div class="cleanup-error">
    <Banner tone="error">{state.cleanupError}</Banner>
    {#if state.cleanupFailureDetails.length > 0}
      <Disclosure summary="Cleanup error details" tone="quiet">
        <ul class="cleanup-failures">
          {#each state.cleanupFailureDetails as message, index (index)}
            <li>{message}</li>
          {/each}
        </ul>
      </Disclosure>
    {/if}
  </div>
{/if}

<style>
  .cleanup-error {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .cleanup-failures {
    padding-left: var(--space-16);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    color: var(--color-err-text);
    word-break: break-all;
  }
</style>
