<script lang="ts">
  import { COMMAND_REGISTRY } from "../lib/commands";

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  const SHORTCUTS = [
    { keys: "Enter", description: "Send message" },
    { keys: "Shift Enter", description: "New line in composer" },
    { keys: "/", description: "Open command menu" },
    { keys: "Esc", description: "Close menus and overlays" },
    { keys: "?", description: "Open this help" },
  ];

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
  <div
    class="help-overlay"
    role="dialog"
    aria-modal="true"
    aria-label="Help"
    tabindex="-1"
    onclick={(e) => {
      if (e.target === e.currentTarget) onClose();
    }}
  >
    <div class="help-content">
      <section class="help-section">
        <h2 class="help-heading">Composing</h2>
        <dl class="help-list">
          {#each SHORTCUTS as item (item.keys)}
            <dt class="help-key">{item.keys}</dt>
            <dd class="help-desc">{item.description}</dd>
          {/each}
        </dl>
      </section>

      <section class="help-section">
        <h2 class="help-heading">Commands</h2>
        <dl class="help-list">
          {#each COMMAND_REGISTRY as cmd (cmd.name)}
            <dt class="help-key">
              {cmd.name}{#if cmd.hasArgs}<span class="help-key-arg"> &lt;text&gt;</span>{/if}
            </dt>
            <dd class="help-desc">{cmd.description}</dd>
          {/each}
        </dl>
      </section>

      <p class="help-dismiss">Press <span class="help-key-inline">Esc</span> to dismiss.</p>
    </div>
  </div>
{/if}

<style>
  .help-overlay {
    position: fixed;
    inset: 0;
    background: rgba(14, 14, 16, 0.92);
    z-index: 300;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--s-6) var(--s-5);
    overflow-y: auto;
    animation: help-in var(--dur-slow) var(--ease-out-stone) forwards;
  }

  .help-content {
    max-width: 640px;
    width: 100%;
  }

  .help-section {
    margin-bottom: var(--s-7);
  }

  .help-heading {
    font-family: var(--font-display);
    font-size: var(--fs-md);
    font-weight: 500;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--text-dim);
    margin-bottom: var(--s-4);
    padding-bottom: var(--s-2);
    border-bottom: 1px solid var(--vein-faint);
    text-shadow:
      0 1px 0 rgba(0, 0, 0, 0.6),
      0 -1px 0 rgba(255, 255, 255, 0.03);
  }

  .help-list {
    display: grid;
    grid-template-columns: minmax(140px, max-content) 1fr;
    gap: var(--s-3) var(--s-5);
    align-items: baseline;
  }

  .help-key {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    color: var(--vein-bright);
    letter-spacing: 0.04em;
  }

  .help-key-arg {
    color: var(--text-dim);
  }

  .help-desc {
    font-family: var(--font-body);
    font-size: var(--fs-base);
    color: var(--text-muted);
    line-height: 1.5;
  }

  .help-dismiss {
    margin-top: var(--s-6);
    text-align: center;
    font-family: var(--font-body);
    font-size: var(--fs-sm);
    color: var(--text-dim);
  }

  .help-key-inline {
    font-family: var(--font-mono);
    color: var(--text-muted);
    padding: 0 var(--s-1);
  }

  @keyframes help-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .help-overlay {
      animation: none;
      opacity: 1;
    }
  }
</style>
