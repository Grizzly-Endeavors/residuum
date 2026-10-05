<script lang="ts">
  import type { SetupWizardState, McpCatalogEntry } from "../../lib/types";
  import { Badge, Banner, Button, EmptyState, Skeleton, TextField } from "../../lib/ui";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    catalog: McpCatalogEntry[];
    catalogLoading: boolean;
    catalogError: string | null;
    onRetryCatalog: () => void;
    onNext: () => void;
    onBack: () => void;
  }

  let {
    wizardState = $bindable(),
    catalog,
    catalogLoading,
    catalogError,
    onRetryCatalog,
    onNext,
    onBack,
  }: Props = $props();

  let pendingIdx = $state<number | null>(null);
  let pendingInputs = $state<Record<string, string>>({});
  let inputErrors = $state<Record<string, boolean>>({});

  function isAdded(name: string): boolean {
    return wizardState.mcpServers.some((s) => s.name === name);
  }

  function envKeyOf(field: string): string {
    return field.startsWith("env.") ? field.slice(4) : field;
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
      pendingInputs = Object.fromEntries(srv.requires_input.map((req) => [req.field, ""]));
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
      const val = (pendingInputs[req.field] ?? "").trim();
      if (!val) {
        inputErrors[req.field] = true;
        hasError = true;
      }
    }
    if (hasError) return;

    // Build env with user values — strip "env." prefix from catalog field names
    const env = { ...(srv.env || {}) };
    for (const req of srv.requires_input) {
      env[envKeyOf(req.field)] = (pendingInputs[req.field] ?? "").trim();
    }

    wizardState.mcpServers.push({
      name: srv.name,
      command: srv.command,
      args: [...(srv.args || [])],
      env: env as Record<string, string>,
      secretEnvKeys: srv.requires_input.map((req) => envKeyOf(req.field)),
    });
    pendingIdx = null;
  }

  function handleCancel() {
    pendingIdx = null;
  }
</script>

{#if catalogError}
  <Banner tone="error">
    {catalogError}
    {#snippet actions()}
      <Button size="sm" onclick={onRetryCatalog}>Try again</Button>
    {/snippet}
  </Banner>
{:else if catalogLoading}
  <SetupGroup>
    <Skeleton lines={4} label="Loading the tool server catalog" />
  </SetupGroup>
{:else if catalog.length === 0}
  <EmptyState
    >The catalog has no tool servers to offer. You can add your own in Settings.</EmptyState
  >
{:else}
  <SetupGroup>
    <ul class="setup-servers">
      {#each catalog as srv, i (srv.name)}
        {@const added = isAdded(srv.name)}
        {@const isPending = pendingIdx === i}
        <li class="setup-server">
          <div class="setup-server-row">
            <div class="setup-server-text">
              <code class="setup-server-name">{srv.name}</code>
              <span class="setup-server-desc">{srv.description}</span>
            </div>
            {#if added}
              <Badge tone="positive" dot>Added</Badge>
              <Button
                variant="quiet"
                size="sm"
                aria-label="Remove {srv.name}"
                onclick={() => handleAdd(i)}>Remove</Button
              >
            {:else if !isPending}
              <Button size="sm" aria-label="Add {srv.name}" onclick={() => handleAdd(i)}>Add</Button
              >
            {/if}
          </div>

          {#if isPending}
            <div class="setup-server-inputs">
              {#each srv.requires_input as req (req.field)}
                <TextField
                  label={req.label}
                  bind:value={pendingInputs[req.field]}
                  autocomplete="off"
                  spellcheck="false"
                  error={inputErrors[req.field] ? "Fill this in to add the server." : undefined}
                  oninput={() => {
                    inputErrors[req.field] = false;
                  }}
                />
              {/each}
              <div class="setup-server-actions">
                <Button variant="primary" size="sm" onclick={() => handleConfirm(i)}>
                  Add {srv.name}
                </Button>
                <Button variant="quiet" size="sm" onclick={handleCancel}>Cancel</Button>
              </div>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  </SetupGroup>
{/if}

<SetupNav {onBack} {onNext} />

<style>
  .setup-servers {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .setup-server {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-12) 0;

    &:first-child {
      padding-top: 0;
    }

    &:last-child {
      padding-bottom: 0;
    }

    & + & {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .setup-server-row {
    display: flex;
    align-items: center;
    gap: var(--space-10);
  }

  .setup-server-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .setup-server-name {
    font-size: var(--font-size-sm);
    overflow-wrap: anywhere;
  }

  .setup-server-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .setup-server-inputs {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-14);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
  }

  .setup-server-actions {
    display: flex;
    gap: var(--space-8);
  }
</style>
