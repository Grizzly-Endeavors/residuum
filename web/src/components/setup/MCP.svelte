<script lang="ts">
  import type { SetupWizardState, McpCatalogEntry } from "../../lib/types";

  interface Props {
    wizardState: SetupWizardState;
    catalog: McpCatalogEntry[];
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState, catalog, onNext, onBack }: Props = $props();

  let pendingIdx = $state<number | null>(null);
  let pendingInputs = $state<Record<string, string>>({});
  let inputErrors = $state<Record<string, boolean>>({});

  function isAdded(name: string): boolean {
    return wizardState.mcpServers.some((s) => s.name === name);
  }

  function setNestedField(
    obj: Record<string, unknown>,
    path: string,
    value: string,
  ) {
    const parts = path.split(".");
    let target: Record<string, unknown> = obj;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!target[parts[i]] || typeof target[parts[i]] !== "object") {
        target[parts[i]] = {};
      }
      target = target[parts[i]] as Record<string, unknown>;
    }
    target[parts[parts.length - 1]] = value;
  }

  function handleAdd(idx: number) {
    const srv = catalog[idx];
    if (!srv) return;

    if (isAdded(srv.name)) {
      // Toggle off
      const existsIdx = wizardState.mcpServers.findIndex((s) => s.name === srv.name);
      if (existsIdx >= 0) wizardState.mcpServers.splice(existsIdx, 1);
      pendingIdx = null;
      return;
    }

    if (srv.requires_input && srv.requires_input.length > 0) {
      pendingIdx = idx;
      pendingInputs = {};
      inputErrors = {};
    } else {
      wizardState.mcpServers.push({
        name: srv.name,
        command: srv.command,
        args: [...(srv.args || [])],
        env: { ...(srv.env || {}) },
      });
    }
  }

  function handleConfirm(idx: number) {
    const srv = catalog[idx];
    if (!srv) return;

    // Validate all required inputs
    let hasError = false;
    for (const req of srv.requires_input) {
      const val = (pendingInputs[req.field] || "").trim();
      if (!val) {
        inputErrors[req.field] = true;
        hasError = true;
      }
    }
    if (hasError) return;

    // Build env with user values
    const env = { ...(srv.env || {}) };
    for (const req of srv.requires_input) {
      setNestedField(env, req.field, pendingInputs[req.field].trim());
    }

    wizardState.mcpServers.push({
      name: srv.name,
      command: srv.command,
      args: [...(srv.args || [])],
      env: env as Record<string, string>,
    });
    pendingIdx = null;
  }

  function handleCancel() {
    pendingIdx = null;
  }
</script>

<h2>MCP Servers</h2>
<p class="subtitle">Optionally add tool servers. You can always add more later in settings.</p>

{#if catalog.length === 0}
  <p style="color:var(--text-dim)">No catalog entries available.</p>
{:else}
  {#each catalog as srv, i}
    {@const added = isAdded(srv.name)}
    {@const isPending = pendingIdx === i}
    <div class="mcp-item" class:added class:pending={isPending}>
      <div class="mcp-info">
        <div class="mcp-name">{srv.name}</div>
        <div class="mcp-desc">{srv.description}</div>
      </div>

      {#if !isPending}
        <button class="mcp-add-btn" onclick={() => handleAdd(i)}>
          {added ? "Added" : "Add"}
        </button>
      {/if}

      {#if isPending && srv.requires_input.length > 0}
        <div class="mcp-inline-inputs">
          {#each srv.requires_input as req}
            <div class="settings-field mcp-input-field">
              <label>{req.label}</label>
              <input
                type="text"
                class:input-error={inputErrors[req.field]}
                bind:value={pendingInputs[req.field]}
                placeholder={req.label}
                oninput={() => { inputErrors[req.field] = false; }}
              />
            </div>
          {/each}
          <div class="mcp-inline-actions">
            <button class="btn btn-primary btn-sm" onclick={() => handleConfirm(i)}>Add</button>
            <button class="btn btn-secondary btn-sm" onclick={handleCancel}>Cancel</button>
          </div>
        </div>
      {/if}
    </div>
  {/each}
{/if}

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack}>Back</button>
  <button class="btn btn-primary" onclick={onNext}>Next</button>
</div>
