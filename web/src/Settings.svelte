<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsSection,
    SettingsMode,
    McpServerEntry,
    SettingsProviderEntry,
    SettingsModelAssignments,
  } from "./lib/types";
  import {
    fetchConfigRaw,
    fetchProvidersRaw,
    fetchMcpRaw,
    putConfigRaw,
    putProvidersRaw,
    putMcpRaw,
    storeSecret,
  } from "./lib/api";
  import {
    parseConfigToml,
    parseProvidersToml,
    parseMcpJson,
    serializeConfigToml,
    serializeProvidersToml,
    serializeMcpJson,
    defaultConfigFields,
    defaultModels,
    type ConfigFields,
  } from "./lib/settings-toml";
  import Runtime from "./components/settings/Runtime.svelte";
  import Providers from "./components/settings/Providers.svelte";
  import Memory from "./components/settings/Memory.svelte";
  import Integrations from "./components/settings/Integrations.svelte";
  import MCP from "./components/settings/MCP.svelte";
  import Modal from "./components/Modal.svelte";
  import { Icon } from "./lib/icons";
  import { toast } from "./lib/toast.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // ── State ──────────────────────────────────────────────────────────

  let activeSection = $state<SettingsSection>("runtime");
  let settingsMode = $state<SettingsMode>(
    (localStorage.getItem("residuum-settings-mode") as SettingsMode) || "simple",
  );
  let loading = $state(true);
  let saving = $state(false);
  let statusMsg = $state("");
  let statusKind = $state<"error" | "success" | "saving" | "">("");
  let initialized = $state(false);

  // Raw text (source of truth from last save/load)
  let rawConfig = $state("");
  let rawProviders = $state("");
  let rawMcp = $state("");

  // Advanced mode editing buffers
  let editConfig = $state("");
  let editProviders = $state("");
  let editMcp = $state("");
  let advancedTab = $state<"config" | "providers" | "mcp">("config");

  // Form state
  let configFields = $state<ConfigFields>(defaultConfigFields());
  let providerEntries = $state<SettingsProviderEntry[]>([]);
  let modelAssignments = $state<SettingsModelAssignments>(defaultModels());
  let mcpServers = $state<McpServerEntry[]>([]);

  // Auto-save debounce timer
  let autoSaveTimer: ReturnType<typeof setTimeout> | undefined;
  let statusClearTimer: ReturnType<typeof setTimeout> | undefined;
  let lastSavedSnapshot = "";

  // ── Sidebar ────────────────────────────────────────────────────────

  let mobileNavOpen = $state(false);

  const sections: { id: SettingsSection; label: string }[] = [
    { id: "runtime", label: "Runtime" },
    { id: "providers", label: "Providers" },
    { id: "memory", label: "Memory" },
    { id: "integrations", label: "Integrations" },
    { id: "mcp", label: "MCP" },
  ];

  let simple = $derived(settingsMode === "simple");

  function activeLabel(): string {
    return sections.find((s) => s.id === activeSection)?.label ?? "Runtime";
  }

  // ── Load ───────────────────────────────────────────────────────────

  onMount(async () => {
    try {
      const [cfgRaw, provRaw, mcpRaw] = await Promise.all([
        fetchConfigRaw(),
        fetchProvidersRaw(),
        fetchMcpRaw(),
      ]);
      rawConfig = cfgRaw;
      rawProviders = provRaw;
      rawMcp = mcpRaw;

      parseAllToForm();
    } catch (err: unknown) {
      statusMsg = `Failed to load settings: ${String(err)}`;
      statusKind = "error";
    } finally {
      loading = false;
      // Set initial snapshot before enabling auto-save
      lastSavedSnapshot = currentSnapshot();
      initialized = true;
    }
  });

  function parseAllToForm() {
    configFields = parseConfigToml(rawConfig);
    const prov = parseProvidersToml(rawProviders);
    providerEntries = prov.providers;
    modelAssignments = prov.models;
    mcpServers = parseMcpJson(rawMcp);
  }

  // ── Mode switching ────────────────────────────────────────────────

  function setMode(mode: SettingsMode) {
    if (mode === settingsMode) return;
    statusMsg = "";
    statusKind = "";

    if (mode === "raw") {
      // Entering raw — load text editors
      editConfig = rawConfig;
      editProviders = rawProviders;
      editMcp = rawMcp;
    } else if (settingsMode === "raw") {
      // Leaving raw — reload form from saved raw state
      parseAllToForm();
    }

    settingsMode = mode;
    localStorage.setItem("residuum-settings-mode", mode);
  }

  // ── Reload ─────────────────────────────────────────────────────────

  let reloadConfirmOpen = $state(false);

  function requestReload() {
    if (currentSnapshot() !== lastSavedSnapshot) {
      reloadConfirmOpen = true;
    } else {
      void handleReload();
    }
  }

  function confirmReload() {
    reloadConfirmOpen = false;
    void handleReload();
  }

  async function handleReload() {
    loading = true;
    statusMsg = "";
    statusKind = "";
    try {
      const [cfgRaw, provRaw, mcpRaw] = await Promise.all([
        fetchConfigRaw(),
        fetchProvidersRaw(),
        fetchMcpRaw(),
      ]);
      rawConfig = cfgRaw;
      rawProviders = provRaw;
      rawMcp = mcpRaw;

      if (settingsMode === "raw") {
        editConfig = rawConfig;
        editProviders = rawProviders;
        editMcp = rawMcp;
      } else {
        parseAllToForm();
      }
      lastSavedSnapshot = currentSnapshot();
      showStatus("Reloaded", "success");
    } catch (err: unknown) {
      toast.error(`Reload failed. ${String(err)}`);
    } finally {
      loading = false;
    }
  }

  // ── Status display ─────────────────────────────────────────────────

  function showStatus(msg: string, kind: "success" | "error") {
    if (statusClearTimer) clearTimeout(statusClearTimer);
    statusMsg = msg;
    statusKind = kind;
    if (kind === "success") {
      statusClearTimer = setTimeout(() => {
        statusMsg = "";
        statusKind = "";
      }, 2000);
    }
  }

  // ── Auto-save ──────────────────────────────────────────────────────

  function currentSnapshot(): string {
    if (settingsMode === "raw") {
      return `adv:${editConfig}|${editProviders}|${editMcp}`;
    }
    return `form:${JSON.stringify($state.snapshot(configFields))}|${JSON.stringify($state.snapshot(providerEntries))}|${JSON.stringify($state.snapshot(modelAssignments))}|${JSON.stringify($state.snapshot(mcpServers))}`;
  }

  function scheduleAutoSave() {
    if (!initialized || saving) return;
    const snap = currentSnapshot();
    if (snap === lastSavedSnapshot) return;
    if (autoSaveTimer) clearTimeout(autoSaveTimer);
    autoSaveTimer = setTimeout(() => {
      void autoSave();
    }, 800);
  }

  // Form mode: watch all form state for changes
  $effect(() => {
    $state.snapshot(configFields);
    $state.snapshot(providerEntries);
    $state.snapshot(modelAssignments);
    $state.snapshot(mcpServers);
    if (settingsMode !== "raw") scheduleAutoSave();
  });

  // Raw mode: watch editor buffers for changes
  $effect(() => {
    editConfig;
    editProviders;
    editMcp;
    if (settingsMode === "raw") scheduleAutoSave();
  });

  async function autoSave(): Promise<void> {
    if (saving) return;
    const snap = currentSnapshot();
    if (snap === lastSavedSnapshot) return;

    saving = true;
    statusMsg = "Saving...";
    statusKind = "saving";

    try {
      let cfgToml: string;
      let provToml: string;
      let mcpJson: string;

      if (settingsMode === "raw") {
        cfgToml = editConfig;
        provToml = editProviders;
        mcpJson = editMcp;
      } else {
        await storeNewSecrets();
        cfgToml = serializeConfigToml(configFields);
        provToml = serializeProvidersToml(providerEntries, modelAssignments);
        mcpJson = serializeMcpJson(mcpServers);
      }

      const provResult = await putProvidersRaw(provToml);
      if (!provResult.valid) {
        statusMsg = "";
        statusKind = "";
        toast.error(`providers.toml: ${provResult.error ?? "unknown error"}`);
        return;
      }

      const cfgResult = await putConfigRaw(cfgToml);
      if (!cfgResult.valid) {
        statusMsg = "";
        statusKind = "";
        toast.error(`config.toml: ${cfgResult.error ?? "unknown error"}`);
        return;
      }

      const mcpResult = await putMcpRaw(mcpJson);
      if (!mcpResult.valid) {
        statusMsg = "";
        statusKind = "";
        toast.error(`mcp.json: ${mcpResult.error ?? "unknown error"}`);
        return;
      }

      rawConfig = cfgToml;
      rawProviders = provToml;
      rawMcp = mcpJson;

      // Record snapshot after save (includes any secret mutations)
      lastSavedSnapshot = currentSnapshot();
      showStatus("Saved", "success");
    } catch (err: unknown) {
      statusMsg = "";
      statusKind = "";
      toast.error(`Save failed. ${String(err)}`);
    } finally {
      saving = false;
    }
  }

  // ── Secret management ──────────────────────────────────────────────

  async function storeNewSecrets() {
    // Collect secrets that need storing (non-empty, non-secret: values)
    const secretOps: {
      field: "discord_token" | "telegram_token" | "cloud_token";
      name: string;
    }[] = [];

    const secretFields = [
      { field: "discord_token" as const, name: "discord" },
      { field: "telegram_token" as const, name: "telegram" },
      { field: "cloud_token" as const, name: "cloud_token" },
    ];

    for (const { field, name } of secretFields) {
      const val = configFields[field];
      if (val && !val.startsWith("secret:")) {
        secretOps.push({ field, name });
      }
    }

    // Webhook secrets
    const webhookSecretOps: { idx: number; name: string }[] = [];
    for (let i = 0; i < configFields.webhooks.length; i++) {
      const wh = configFields.webhooks[i];
      if (wh?.secret && !wh.secret.startsWith("secret:")) {
        webhookSecretOps.push({ idx: i, name: `webhook_${wh.name}` });
      }
    }

    // Store provider API keys
    const provKeyOps: { idx: number; name: string }[] = [];
    for (let i = 0; i < providerEntries.length; i++) {
      const p = providerEntries[i];
      if (p?.apiKey && !p.apiKey.startsWith("secret:") && p.type !== "ollama") {
        provKeyOps.push({ idx: i, name: p.name });
      }
    }

    // Execute all secret stores
    for (const { field, name } of secretOps) {
      const result = await storeSecret(name, configFields[field]);
      configFields[field] = result.reference;
    }

    for (const { idx, name } of webhookSecretOps) {
      const wh = configFields.webhooks[idx];
      if (!wh) continue;
      const result = await storeSecret(name, wh.secret);
      wh.secret = result.reference;
    }

    for (const { idx, name } of provKeyOps) {
      const entry = providerEntries[idx];
      if (!entry) continue;
      const result = await storeSecret(name, entry.apiKey);
      entry.apiKey = result.reference;
    }
  }
</script>

<div class="settings-view emerges">
  <div class="settings-header">
    <span class="settings-title">Settings</span>
    <div class="settings-header-actions">
      {#if statusMsg}
        <span class="settings-status {statusKind}">{statusMsg}</span>
      {/if}
      <button
        class="icon-btn"
        title="Reload from disk"
        aria-label="Reload from disk"
        onclick={requestReload}
        disabled={saving}
      >
        <Icon name="reload" size={16} />
      </button>
      <div class="settings-mode-selector">
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "simple"}
          onclick={() => setMode("simple")}
        >
          Simple
        </button>
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "advanced"}
          onclick={() => setMode("advanced")}
        >
          Advanced
        </button>
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "raw"}
          onclick={() => setMode("raw")}
        >
          Raw
        </button>
      </div>
      <button class="icon-btn" title="Close settings" aria-label="Close settings" onclick={onClose}>
        <Icon name="close" size={16} />
      </button>
    </div>
  </div>

  <div class="settings-body">
    {#if settingsMode !== "raw"}
      <div class="settings-sidebar" class:collapsed={!mobileNavOpen}>
        <button
          class="settings-nav-toggle"
          onclick={() => {
            mobileNavOpen = !mobileNavOpen;
          }}
        >
          <span>{activeLabel()}</span>
          <span class="nav-chevron" class:open={mobileNavOpen}>&#9660;</span>
        </button>
        <div class="settings-nav-items">
          {#each sections as sec (sec.id)}
            <button
              class="settings-sidebar-btn"
              class:active={activeSection === sec.id}
              onclick={() => {
                activeSection = sec.id;
                mobileNavOpen = false;
              }}
            >
              {sec.label}
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <div class="settings-content">
      {#if loading}
        <p style="color:var(--text-dim); padding:20px;">Loading settings...</p>
      {:else if settingsMode === "raw"}
        <!-- Raw tabbed editor -->
        <div class="advanced-tabs">
          <button
            class="advanced-tab"
            class:active={advancedTab === "config"}
            onclick={() => {
              advancedTab = "config";
            }}>config.toml</button
          >
          <button
            class="advanced-tab"
            class:active={advancedTab === "providers"}
            onclick={() => {
              advancedTab = "providers";
            }}>providers.toml</button
          >
          <button
            class="advanced-tab"
            class:active={advancedTab === "mcp"}
            onclick={() => {
              advancedTab = "mcp";
            }}>mcp.json</button
          >
        </div>
        {#if advancedTab === "config"}
          <textarea class="toml-editor" bind:value={editConfig}></textarea>
        {:else if advancedTab === "providers"}
          <textarea class="toml-editor" bind:value={editProviders}></textarea>
        {:else}
          <textarea class="toml-editor" bind:value={editMcp}></textarea>
        {/if}
      {:else if activeSection === "runtime"}
        <Runtime bind:fields={configFields} {simple} />
      {:else if activeSection === "providers"}
        <Providers bind:providers={providerEntries} bind:models={modelAssignments} />
      {:else if activeSection === "memory"}
        <Memory bind:fields={configFields} {simple} />
      {:else if activeSection === "integrations"}
        <Integrations bind:fields={configFields} {simple} />
      {:else if activeSection === "mcp"}
        <MCP bind:servers={mcpServers} />
      {/if}
    </div>
  </div>
</div>

<Modal
  open={reloadConfirmOpen}
  title="Discard unsaved changes?"
  onClose={() => {
    reloadConfirmOpen = false;
  }}
>
  Reloading from disk will discard any unsaved edits in this session.

  {#snippet actions()}
    <button
      class="btn btn-secondary"
      onclick={() => {
        reloadConfirmOpen = false;
      }}>Cancel</button
    >
    <button class="btn btn-danger" onclick={confirmReload}>Discard and reload</button>
  {/snippet}
</Modal>
