<script lang="ts">
  import { Icon } from "../../lib/icons";
  import { Button } from "../../lib/ui";
  import { discardScope, saveScope, undoSave, unsavedFailure } from "./scope-actions";
  import type { SettingsScope } from "./sections";

  // The save bar: there while the scope holds staged changes,
  // with Save changes and Discard. When a save left files unsaved it names
  // them here until those changes are saved or discarded, with Undo for the
  // files that did save.

  let { scope }: { scope: SettingsScope } = $props();

  const failure = $derived(unsavedFailure(scope));
</script>

{#if scope.dirty || scope.saving}
  <div class="save-bar" role="region" aria-label="Unsaved changes" data-failed={failure !== null}>
    {#if scope.saving}
      <p class="save-message" role="status">Saving…</p>
    {:else if failure !== null}
      <p class="save-message" role="alert">
        <span class="save-icon"><Icon name="warning" size={16} /></span>{failure.message}
      </p>
    {:else}
      <p class="save-message" role="status">You have unsaved changes.</p>
    {/if}
    <div class="save-actions">
      {#if failure !== null && scope.undoable}
        <Button size="sm" variant="quiet" onclick={() => void undoSave(scope)}>Undo</Button>
      {/if}
      <Button size="sm" variant="quiet" disabled={scope.saving} onclick={() => discardScope(scope)}>
        Discard
      </Button>
      <Button
        size="sm"
        variant="primary"
        loading={scope.saving}
        onclick={() => void saveScope(scope)}
      >
        Save changes
      </Button>
    </div>
  </div>
{/if}

<style>
  .save-bar {
    position: sticky;
    bottom: var(--space-16);
    z-index: var(--z-sticky);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    max-width: 640px;
    margin-top: var(--space-8);
    padding: var(--space-8) var(--space-8) var(--space-8) var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    font-size: var(--font-size-sm);
    animation: save-bar-in var(--duration-base) var(--ease-out);
  }

  .save-message {
    display: flex;
    flex: 1 1 220px;
    gap: var(--space-8);
    color: var(--color-text-2);
  }

  [data-failed="true"] .save-message {
    color: var(--color-text);
  }

  .save-icon {
    display: grid;
    flex: none;
    padding-top: var(--space-2);
    color: var(--color-err-text);
  }

  .save-actions {
    display: flex;
    gap: var(--space-6);
    margin-left: auto;
  }

  @keyframes save-bar-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
