<script lang="ts">
  import { toast } from "../lib/toast.svelte";
</script>

<div class="toast-host" role="status" aria-live="polite">
  {#each [...toast.toasts.values()] as t (t.id)}
    <button
      type="button"
      class="toast toast-{t.kind}"
      onclick={() => toast.dismiss(t.id)}
      title="Click to dismiss"
    >
      {t.message}
    </button>
  {/each}
</div>

<style>
  .toast-host {
    position: fixed;
    bottom: var(--s-5);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--s-2);
    z-index: 100;
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    max-width: min(440px, calc(100vw - 32px));
    padding: var(--s-3) var(--s-4);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: var(--font-body);
    font-size: var(--fs-md);
    line-height: 1.5;
    text-align: center;
    cursor: pointer;
    box-shadow: var(--elev-3);
    animation: toast-emerge var(--dur-default) var(--ease-out-stone) forwards;
  }

  .toast-success {
    border-color: rgba(46, 204, 113, 0.32);
  }

  .toast-error {
    border-color: rgba(192, 57, 43, 0.5);
    color: #e8a0a0;
  }

  .toast-info {
    border-color: var(--vein-dim);
  }

  @keyframes toast-emerge {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .toast {
      animation: none;
      opacity: 1;
    }
  }
</style>
