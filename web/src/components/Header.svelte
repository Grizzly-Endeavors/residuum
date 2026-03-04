<script lang="ts">
  import type { ConnectionStatus } from "../lib/types";

  let { status, verbose, onToggleVerbose }: {
    status: ConnectionStatus;
    verbose: boolean;
    onToggleVerbose: () => void;
  } = $props();
</script>

<div class="header">
  <div class="header-brand">
    <div class="header-icon">&#9670;</div>
    <span class="header-title">Residuum</span>
    <span class="header-status {status}">{status}</span>
  </div>
  <div class="header-actions">
    <button
      class="icon-btn"
      class:active={verbose}
      title="Toggle tool visibility"
      onclick={onToggleVerbose}
    >
      &#128295;
    </button>
  </div>
</div>
