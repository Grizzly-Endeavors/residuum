<script lang="ts">
  import { onMount } from "svelte";
  import type { McpServerEntry, McpCatalogEntry } from "../../lib/types";
  import { fetchMcpCatalog } from "../../lib/api";

  let { servers = $bindable() }: { servers: McpServerEntry[] } = $props();

  let catalog = $state<McpCatalogEntry[]>([]);
  let pendingIdx = $state<number | null>(null);
  let pendingInputs = $state<Record<string, string>>({});
  let inputErrors = $state<Record<string, boolean>>({});

  // Manual add form
  let showAddForm = $state(false);
  let newServer = $state<McpServerEntry>({ name: "", command: "", args: [], env: {} });
  let newArgsStr = $state("");
  let newEnvStr = $state("");

  onMount(async () => {
    catalog = await fetchMcpCatalog();
  });

  function removeServer(idx: number) {
    servers.splice(idx, 1);
  }

  // ── Catalog handling ─────────────────────────────────────────────────

  function isAdded(name: string): boolean {
    return servers.some((s) => s.name === name);
  }

  function handleCatalogAdd(idx: number) {
    const srv = catalog[idx];
    if (!srv) return;

    if (isAdded(srv.name)) {
      const existsIdx = servers.findIndex((s) => s.name === srv.name);
      if (existsIdx >= 0) servers.splice(existsIdx, 1);
      pendingIdx = null;
      return;
    }

    if (srv.requires_input && srv.requires_input.length > 0) {
      pendingIdx = idx;
      pendingInputs = {};
      inputErrors = {};
    } else {
      servers.push({
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

    let hasError = false;
    for (const req of srv.requires_input) {
      const val = (pendingInputs[req.field] ?? "").trim();
      if (!val) {
        inputErrors[req.field] = true;
        hasError = true;
      }
    }
    if (hasError) return;

    const env = { ...(srv.env || {}) };
    for (const req of srv.requires_input) {
      const key = req.field.startsWith("env.") ? req.field.slice(4) : req.field;
      env[key] = (pendingInputs[req.field] ?? "").trim();
    }

    servers.push({
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

  // ── Manual add ─────────────────────────────────────────────────────

  function handleManualAdd() {
    if (!newServer.name.trim() || !newServer.command.trim()) return;
    const args = newArgsStr.trim() ? newArgsStr.trim().split(/\s+/) : [];
    const env: Record<string, string> = {};
    if (newEnvStr.trim()) {
      for (const line of newEnvStr.trim().split("\n")) {
        const eq = line.indexOf("=");
        if (eq > 0) {
          env[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
        }
      }
    }
    servers.push({
      name: newServer.name.trim(),
      command: newServer.command.trim(),
      args,
      env,
    });
    newServer = { name: "", command: "", args: [], env: {} };
    newArgsStr = "";
    newEnvStr = "";
    showAddForm = false;
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Configured Servers</div>

    {#if servers.length === 0}
      <p style="color:var(--text-dim); font-size:13px;">No MCP servers configured.</p>
    {/if}

    {#each servers as srv, i (srv.name)}
      <div class="mcp-server-entry">
        <div class="mcp-server-info">
          <span class="mcp-server-name">{srv.name}</span>
          <span class="mcp-server-cmd">{srv.command} {srv.args.join(" ")}</span>
        </div>
        <button class="btn btn-sm btn-danger" onclick={() => removeServer(i)}>Remove</button>
      </div>
    {/each}

    {#if showAddForm}
      <div class="mcp-add-form">
        <div class="settings-field">
          <label for="mcp-new-name">Name</label>
          <input
            id="mcp-new-name"
            type="text"
            bind:value={newServer.name}
            placeholder="Server name"
          />
        </div>
        <div class="settings-field">
          <label for="mcp-new-command">Command</label>
          <input
            id="mcp-new-command"
            type="text"
            bind:value={newServer.command}
            placeholder="e.g. npx, uvx"
          />
        </div>
        <div class="settings-field">
          <label for="mcp-new-args">Arguments (space-separated)</label>
          <input
            id="mcp-new-args"
            type="text"
            bind:value={newArgsStr}
            placeholder="e.g. -y @org/server"
          />
        </div>
        <div class="settings-field">
          <label for="mcp-new-env">Environment (KEY=value, one per line)</label>
          <textarea
            id="mcp-new-env"
            class="toml-editor"
            style="min-height:60px;"
            bind:value={newEnvStr}
            placeholder="API_KEY=abc123"
          ></textarea>
        </div>
        <div class="mcp-inline-actions">
          <button class="btn btn-primary btn-sm" onclick={handleManualAdd}>Add</button>
          <button
            class="btn btn-secondary btn-sm"
            onclick={() => {
              showAddForm = false;
            }}>Cancel</button
          >
        </div>
      </div>
    {:else}
      <button
        class="btn btn-secondary btn-sm"
        style="margin-top:8px;"
        onclick={() => {
          showAddForm = true;
        }}>+ Add Server</button
      >
    {/if}
  </div>

  <!-- Catalog Browser -->
  <div class="settings-group">
    <div class="settings-group-label">Catalog</div>
    <p class="roles-section-hint">Browse available MCP servers. Click to add or remove.</p>

    {#if catalog.length === 0}
      <p style="color:var(--text-dim); font-size:13px;">Loading catalog...</p>
    {:else}
      {#each catalog as srv, i (srv.name)}
        {@const added = isAdded(srv.name)}
        {@const isPending = pendingIdx === i}
        <div class="mcp-item" class:added class:pending={isPending}>
          <div class="mcp-info">
            <div class="mcp-name">{srv.name}</div>
            <div class="mcp-desc">{srv.description}</div>
          </div>

          {#if !isPending}
            <button class="mcp-add-btn" onclick={() => handleCatalogAdd(i)}>
              {added ? "Added" : "Add"}
            </button>
          {/if}

          {#if isPending && srv.requires_input.length > 0}
            <div class="mcp-inline-inputs">
              {#each srv.requires_input as req (req.field)}
                <div class="settings-field mcp-input-field">
                  <label for="mcp-input-{i}-{req.field}">{req.label}</label>
                  <input
                    id="mcp-input-{i}-{req.field}"
                    type="text"
                    class:input-error={inputErrors[req.field]}
                    bind:value={pendingInputs[req.field]}
                    placeholder={req.label}
                    oninput={() => {
                      inputErrors[req.field] = false;
                    }}
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
  </div>
</div>
