<script lang="ts">
  import { onMount } from "svelte";
  import type { McpServerEntry, McpCatalogEntry } from "../../lib/types";
  import { fetchMcpCatalog } from "../../lib/api";
  import { notifyFormUndo } from "../../lib/form-undo";
  import type { PendingSaveTracker } from "../../lib/pending-save";

  let {
    servers = $bindable(),
    pendingSave,
    onReload,
  }: {
    servers: McpServerEntry[];
    pendingSave: PendingSaveTracker;
    onReload: () => Promise<void>;
  } = $props();

  let catalog = $state<McpCatalogEntry[]>([]);
  let pendingIdx = $state<number | null>(null);
  let pendingInputs = $state<Record<string, string>>({});
  let inputErrors = $state<Record<string, boolean>>({});

  // Manual add form
  let showAddForm = $state(false);
  let newServer = $state<McpServerEntry>({
    name: "",
    transport: "stdio",
    command: "",
    args: [],
    env: {},
    url: "",
    headers: {},
  });
  let newArgsStr = $state("");
  let newEnvStr = $state("");
  let newHeadersStr = $state("");

  function transportOf(srv: McpServerEntry): "stdio" | "http" {
    return srv.transport ?? "stdio";
  }

  onMount(async () => {
    catalog = await fetchMcpCatalog();
  });

  function removeServer(idx: number) {
    const [removed] = servers.splice(idx, 1);
    if (!removed) return;
    notifyFormUndo(
      `Removed ${removed.name}.`,
      pendingSave,
      () => {
        servers.splice(idx, 0, removed);
      },
      "workspace",
      "config/mcp.json",
      onReload,
    );
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

  /** Parse `KEY=value` (env) or `Header-Name=value` (headers) lines, one per line. */
  function parseKvLines(raw: string): Record<string, string> {
    const out: Record<string, string> = {};
    if (!raw.trim()) return out;
    for (const line of raw.trim().split("\n")) {
      const eq = line.indexOf("=");
      if (eq > 0) {
        out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
      }
    }
    return out;
  }

  function handleManualAdd() {
    const name = newServer.name.trim();
    if (!name) return;

    if (newServer.transport === "http") {
      const url = (newServer.url ?? "").trim();
      if (!url) return;
      servers.push({
        name,
        transport: "http",
        command: "",
        args: [],
        env: {},
        url,
        headers: parseKvLines(newHeadersStr),
      });
    } else {
      const command = newServer.command.trim();
      if (!command) return;
      servers.push({
        name,
        transport: "stdio",
        command,
        args: newArgsStr.trim() ? newArgsStr.trim().split(/\s+/) : [],
        env: parseKvLines(newEnvStr),
      });
    }

    newServer = {
      name: "",
      transport: "stdio",
      command: "",
      args: [],
      env: {},
      url: "",
      headers: {},
    };
    newArgsStr = "";
    newEnvStr = "";
    newHeadersStr = "";
    showAddForm = false;
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Configured Servers</div>

    {#if servers.length === 0}
      <p class="empty-state">No MCP servers configured.</p>
    {/if}

    {#each servers as srv, i (srv.name)}
      <div class="mcp-server-entry">
        <div class="mcp-server-info">
          <span class="mcp-server-name">{srv.name}</span>
          {#if transportOf(srv) === "http"}
            <span class="mcp-server-cmd">http · {srv.url}</span>
          {:else}
            <span class="mcp-server-cmd">{srv.command} {srv.args.join(" ")}</span>
          {/if}
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
          <span class="settings-group-label">Transport</span>
          <div class="settings-mode-selector">
            <button
              type="button"
              class="settings-mode-btn"
              class:active={newServer.transport === "stdio"}
              onclick={() => {
                newServer.transport = "stdio";
              }}>stdio</button
            >
            <button
              type="button"
              class="settings-mode-btn"
              class:active={newServer.transport === "http"}
              onclick={() => {
                newServer.transport = "http";
              }}>http</button
            >
          </div>
        </div>
        {#if newServer.transport === "http"}
          <div class="settings-field">
            <label for="mcp-new-url">URL</label>
            <input
              id="mcp-new-url"
              type="text"
              bind:value={newServer.url}
              placeholder="https://mcp.example.com/v1"
            />
          </div>
          <div class="settings-field">
            <label for="mcp-new-headers">Headers (Header-Name=value, one per line)</label>
            <textarea
              id="mcp-new-headers"
              class="toml-editor"
              style="min-height:60px;"
              bind:value={newHeadersStr}
              placeholder="Authorization=Bearer token123"
            ></textarea>
          </div>
        {:else}
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
        {/if}
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
      <p class="empty-state">Reading catalog.</p>
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
