<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    title: string;
    open: boolean;
    onClose: () => void;
    children: Snippet;
    actions?: Snippet;
  }

  let { title, open, onClose, children, actions }: Props = $props();
  const titleId = `modal-title-${crypto.randomUUID()}`;

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && open) {
      event.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="modal-backdrop"
    onclick={(e) => {
      if (e.target === e.currentTarget) onClose();
    }}
  >
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby={titleId}>
      <h2 id={titleId} class="modal-title">{title}</h2>
      <div class="modal-body">
        {@render children()}
      </div>
      {#if actions}
        <div class="modal-actions">
          {@render actions()}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(14, 14, 16, 0.45);
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    animation: modal-backdrop-in var(--dur-default) var(--ease-out-stone) forwards;
  }

  .modal {
    max-width: min(440px, calc(100vw - 32px));
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--s-5) var(--s-5) var(--s-4);
    box-shadow:
      var(--elev-3),
      0 0 0 1px var(--vein-faint);
    animation: modal-in var(--dur-slow) var(--ease-out-stone) forwards;
  }

  .modal-title {
    font-family: var(--font-display);
    font-size: var(--fs-lg);
    font-weight: 500;
    letter-spacing: 0.08em;
    margin-bottom: var(--s-3);
    text-shadow:
      0 1px 0 rgba(0, 0, 0, 0.6),
      0 -1px 0 rgba(255, 255, 255, 0.03);
  }

  .modal-body {
    font-family: var(--font-body);
    font-size: var(--fs-base);
    line-height: 1.6;
    color: var(--text-muted);
    margin-bottom: var(--s-5);
  }

  .modal-actions {
    display: flex;
    gap: var(--s-2);
    justify-content: flex-end;
  }

  @keyframes modal-backdrop-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes modal-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .modal-backdrop,
    .modal {
      animation: none;
      opacity: 1;
    }
  }
</style>
