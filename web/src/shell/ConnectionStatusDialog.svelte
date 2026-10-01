<script lang="ts">
  import {
    connectionStatusRows,
    connectionStatusText,
    type StatusRow,
  } from "../lib/connection-status";
  import { userErrorMessage } from "../lib/errors";
  import type { AgentDisplayState } from "../lib/agent-display-state";
  import { toast } from "../lib/toast.svelte";
  import type { ConnectionStatus } from "../lib/types";
  import { Button, Dialog } from "../lib/ui";
  import { readMainModelLabel } from "../places/chat/main-model.svelte";

  // What the old /status line reported, as a dialog: Residuum, the bound
  // agent, that agent's connection, and its main model. The text is selectable,
  // and Copy puts the same lines on the clipboard.

  let {
    open = $bindable(false),
    agent,
    agentState,
    hubConnection,
    agentConnection,
  }: {
    open?: boolean;
    agent: string | null;
    agentState: AgentDisplayState | null;
    hubConnection: ConnectionStatus;
    agentConnection: ConnectionStatus;
  } = $props();

  const rows = $derived(
    connectionStatusRows({ agent, state: agentState, hubConnection, agentConnection }),
  );

  let model = $state<string | null>(null);
  let modelError = $state<string | null>(null);

  $effect(() => {
    if (!open || agent === null) {
      model = null;
      modelError = null;
      return;
    }
    const name = agent;
    let cancelled = false;
    model = null;
    modelError = null;
    void readMainModelLabel(name).then(
      (label) => {
        if (!cancelled) model = label;
      },
      (err: unknown) => {
        if (!cancelled)
          modelError = userErrorMessage(err, { action: "Couldn't read the main model." });
      },
    );
    return () => {
      cancelled = true;
    };
  });

  const modelReady = $derived(agent === null || model !== null || modelError !== null);

  const shown = $derived.by((): StatusRow[] => {
    if (agent === null) return rows;
    const value = modelError ?? model ?? "Loading…";
    return [...rows, { label: "Model", value }];
  });

  let copied = $state(false);

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(connectionStatusText(shown));
      copied = true;
    } catch {
      copied = false;
      toast.error("Couldn't copy the status. Select it and copy it instead.");
    }
  }
</script>

<Dialog
  bind:open
  title="Connection status"
  description="Whether Residuum and the agent are reachable, and which model it uses."
  size="sm"
  onclose={() => (copied = false)}
>
  <dl class="status">
    {#each shown as row (row.label)}
      <div class="status-row">
        <dt>{row.label}</dt>
        <dd class:is-error={row.label === "Model" && modelError !== null}>{row.value}</dd>
      </div>
    {/each}
  </dl>
  {#snippet actions()}
    <Button variant="primary" disabled={!modelReady} onclick={() => void copy()}>
      {copied ? "Copied" : "Copy"}
    </Button>
  {/snippet}
</Dialog>

<style>
  .status-row {
    display: grid;
    grid-template-columns: 7.5rem minmax(0, 1fr);
    gap: var(--space-12);
    align-items: baseline;
    padding: var(--space-6) 0;
  }

  .status-row + .status-row {
    border-top: 1px solid var(--color-line-soft);
  }

  dt {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
  }

  dd {
    margin: 0;
    color: var(--color-text);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    user-select: text;
  }

  .is-error {
    color: var(--color-err-text);
  }
</style>
