<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsSection,
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
    validateConfig,
    validateProviders,
    storeSecret,
    listSecrets,
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
  import WebSearch from "./components/settings/WebSearch.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // ── State ──────────────────────────────────────────────────────────

  let activeSection = $state<SettingsSection>("runtime");
  let advancedMode = $state(false);
  let loading = $state(true);
  let saving = $state(false);
  let validationMsg = $state("");
  let validationKind = $state<"error" | "success" | "">("");

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

  // Secrets tracking
  let _existingSecrets = $state<string[]>([]);

  // ── Sidebar ────────────────────────────────────────────────────────

  let mobileNavOpen = $state(false);

  const sections: { id: SettingsSection; label: string }[] = [
    { id: "runtime", label: "Runtime" },
    { id: "providers", label: "Providers" },
    { id: "memory", label: "Memory" },
    { id: "integrations", label: "Integrations" },
    { id: "mcp", label: "MCP" },
    { id: "web_search", label: "Web Search" },
  ];

  function activeLabel(): string {
    return sections.find((s) => s.id === activeSection)?.label ?? "Runtime";
  }

  // ── Load ───────────────────────────────────────────────────────────

  onMount(async () => {
    try {
      const [cfgRaw, provRaw, mcpRaw, secrets] = await Promise.all([
        fetchConfigRaw(),
        fetchProvidersRaw(),
        fetchMcpRaw(),
        listSecrets(),
      ]);
      rawConfig = cfgRaw;
      rawProviders = provRaw;
      rawMcp = mcpRaw;
      _existingSecrets = secrets;

      parseAllToForm();
    } catch (err: unknown) {
      validationMsg = `Failed to load settings: ${String(err)}`;
      validationKind = "error";
    } finally {
      loading = false;
    }
  });

  function parseAllToForm() {
    configFields = parseConfigToml(rawConfig);
    const prov = parseProvidersToml(rawProviders);
    providerEntries = prov.providers;
    modelAssignments = prov.models;
    mcpServers = parseMcpJson(rawMcp);
  }

  // ── Mode toggle ────────────────────────────────────────────────────

  function toggleMode() {
    validationMsg = "";
    validationKind = "";
    if (advancedMode) {
      // Switching to form mode — reload from saved raw state
      parseAllToForm();
      advancedMode = false;
    } else {
      // Switching to advanced — load raw text into editors
      editConfig = rawConfig;
      editProviders = rawProviders;
      editMcp = rawMcp;
      advancedMode = true;
    }
  }

  // ── Reload ─────────────────────────────────────────────────────────

  async function handleReload() {
    loading = true;
    validationMsg = "";
    validationKind = "";
    try {
      const [cfgRaw, provRaw, mcpRaw] = await Promise.all([
        fetchConfigRaw(),
        fetchProvidersRaw(),
        fetchMcpRaw(),
      ]);
      rawConfig = cfgRaw;
      rawProviders = provRaw;
      rawMcp = mcpRaw;

      if (advancedMode) {
        editConfig = rawConfig;
        editProviders = rawProviders;
        editMcp = rawMcp;
      } else {
        parseAllToForm();
      }
      validationMsg = "Reloaded from disk.";
      validationKind = "success";
    } catch (err: unknown) {
      validationMsg = `Reload failed: ${String(err)}`;
      validationKind = "error";
    } finally {
      loading = false;
    }
  }

  // ── Validate ───────────────────────────────────────────────────────

  async function handleValidate() {
    validationMsg = "";
    validationKind = "";
    saving = true;

    try {
      let cfgToml: string;
      let provToml: string;

      if (advancedMode) {
        cfgToml = editConfig;
        provToml = editProviders;
      } else {
        cfgToml = serializeConfigToml(configFields);
        provToml = serializeProvidersToml(providerEntries, modelAssignments);
      }

      // Validate providers first (config validation reads it from disk)
      const provResult = await validateProviders(provToml);
      if (!provResult.valid) {
        validationMsg = `providers.toml: ${provResult.error ?? "unknown error"}`;
        validationKind = "error";
        return;
      }

      const cfgResult = await validateConfig(cfgToml);
      if (!cfgResult.valid) {
        validationMsg = `config.toml: ${cfgResult.error ?? "unknown error"}`;
        validationKind = "error";
        return;
      }

      validationMsg = "Configuration is valid.";
      validationKind = "success";
    } catch (err: unknown) {
      validationMsg = `Validation error: ${String(err)}`;
      validationKind = "error";
    } finally {
      saving = false;
    }
  }

  // ── Save ───────────────────────────────────────────────────────────

  async function handleSave() {
    validationMsg = "";
    validationKind = "";
    saving = true;

    try {
      let cfgToml: string;
      let provToml: string;
      let mcpJson: string;

      if (advancedMode) {
        cfgToml = editConfig;
        provToml = editProviders;
        mcpJson = editMcp;
      } else {
        // Store new secrets before serializing
        await storeNewSecrets();
        cfgToml = serializeConfigToml(configFields);
        provToml = serializeProvidersToml(providerEntries, modelAssignments);
        mcpJson = serializeMcpJson(mcpServers);
      }

      // Save providers.toml first (config validation reads it from disk)
      const provResult = await putProvidersRaw(provToml);
      if (!provResult.valid) {
        validationMsg = `providers.toml: ${provResult.error ?? "unknown error"}`;
        validationKind = "error";
        return;
      }

      const cfgResult = await putConfigRaw(cfgToml);
      if (!cfgResult.valid) {
        validationMsg = `config.toml: ${cfgResult.error ?? "unknown error"}`;
        validationKind = "error";
        return;
      }

      const mcpResult = await putMcpRaw(mcpJson);
      if (!mcpResult.valid) {
        validationMsg = `mcp.json: ${mcpResult.error ?? "unknown error"}`;
        validationKind = "error";
        return;
      }

      // Update raw state to match what was saved
      rawConfig = cfgToml;
      rawProviders = provToml;
      rawMcp = mcpJson;

      validationMsg = "Settings saved and applied.";
      validationKind = "success";
    } catch (err: unknown) {
      validationMsg = `Save failed: ${String(err)}`;
      validationKind = "error";
    } finally {
      saving = false;
    }
  }

  // ── Secret management ──────────────────────────────────────────────

  async function storeNewSecrets() {
    // Collect secrets that need storing (non-empty, non-secret: values)
    const secretOps: {
      field: "discord_token" | "telegram_token" | "webhook_secret";
      name: string;
    }[] = [];

    const secretFields = [
      { field: "discord_token" as const, name: "discord" },
      { field: "telegram_token" as const, name: "telegram" },
      { field: "webhook_secret" as const, name: "webhook_secret" },
    ];

    for (const { field, name } of secretFields) {
      const val = configFields[field];
      if (val && !val.startsWith("secret:")) {
        secretOps.push({ field, name });
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

    for (const { idx, name } of provKeyOps) {
      const entry = providerEntries[idx];
      if (!entry) continue;
      const result = await storeSecret(name, entry.apiKey);
      entry.apiKey = result.reference;
    }
  }
</script>

<div class="settings-view">
  <div class="settings-header">
    <span class="settings-title">Settings</span>
    <div class="settings-header-actions">
      <button
        class="btn btn-sm"
        class:btn-primary={advancedMode}
        class:btn-secondary={!advancedMode}
        onclick={toggleMode}
      >
        {advancedMode ? "Form Mode" : "Advanced"}
      </button>
      <button class="icon-btn" title="Close settings" onclick={onClose}>&#10005;</button>
    </div>
  </div>

  <div class="settings-body">
    {#if !advancedMode}
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
      {:else if advancedMode}
        <!-- Advanced tabbed editor -->
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
        <Runtime bind:fields={configFields} />
      {:else if activeSection === "providers"}
        <Providers bind:providers={providerEntries} bind:models={modelAssignments} />
      {:else if activeSection === "memory"}
        <Memory bind:fields={configFields} />
      {:else if activeSection === "integrations"}
        <Integrations bind:fields={configFields} />
      {:else if activeSection === "mcp"}
        <MCP bind:servers={mcpServers} />
      {:else if activeSection === "web_search"}
        <WebSearch bind:fields={configFields} />
      {/if}
    </div>
  </div>

  <div class="settings-footer">
    <div class="settings-footer-left">
      <button class="btn btn-secondary" onclick={handleReload} disabled={saving}>Reload</button>
      <button class="btn btn-secondary" onclick={handleValidate} disabled={saving}>Validate</button>
    </div>
    <div class="settings-footer-right">
      {#if validationMsg}
        <span class="validation-msg {validationKind}">{validationMsg}</span>
      {/if}
      <button class="btn btn-primary" onclick={handleSave} disabled={saving}>
        {saving ? "Saving..." : "Save"}
      </button>
    </div>
  </div>
</div>
