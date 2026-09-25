<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsSection,
    SettingsMode,
    McpServerEntry,
    SettingsProviderEntry,
    SettingsModelAssignments,
    Diagnostic,
  } from "./lib/types";
  import {
    fetchConfigRaw,
    fetchProvidersRaw,
    fetchMcpRaw,
    putConfigRaw,
    putProvidersRaw,
    putMcpRaw,
    patchConfig,
    patchProviders,
    patchMcp,
    storeSecret,
    validateConfig,
    validateProviders,
    validateWorkspaceFile,
  } from "./lib/api";
  import { isStoredReference } from "./lib/secrets";
  import { formatDiagnosticLocation } from "./lib/diagnostics";
  import {
    parseConfigToml,
    parseProvidersToml,
    parseMcpJson,
    diffConfigFields,
    diffProviders,
    diffMcpServers,
    defaultConfigFields,
    defaultModels,
    type ConfigFields,
  } from "./lib/settings-toml";
  import Runtime from "./components/settings/Runtime.svelte";
  import Providers from "./components/settings/Providers.svelte";
  import Memory from "./components/settings/Memory.svelte";
  import Integrations from "./components/settings/Integrations.svelte";
  import MCP from "./components/settings/MCP.svelte";
  import AgentKeys from "./components/settings/AgentKeys.svelte";
  import A2a from "./components/settings/A2a.svelte";
  import History from "./components/settings/History.svelte";
  import Modal from "./components/Modal.svelte";
  import { Icon } from "./lib/icons";
  import { toast } from "./lib/toast.svelte";
  import { userErrorMessage } from "./lib/errors";

  let {
    section: activeSection,
    onSelectSection,
    onClose,
  }: {
    section: SettingsSection;
    onSelectSection: (section: SettingsSection) => void;
    onClose: () => void;
  } = $props();

  // ── State ──────────────────────────────────────────────────────────

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

  // Live diagnostics for the raw editors, refreshed on a debounce while
  // typing and replaced with each save's own diagnostics after saving.
  let rawDiagnostics = $state<{ config: Diagnostic[]; providers: Diagnostic[]; mcp: Diagnostic[] }>(
    { config: [], providers: [], mcp: [] },
  );
  let activeRawDiagnostics = $derived(rawDiagnostics[advancedTab]);

  // Form state
  let configFields = $state<ConfigFields>(defaultConfigFields());
  let providerEntries = $state<SettingsProviderEntry[]>([]);
  let modelAssignments = $state<SettingsModelAssignments>(defaultModels());
  let mcpServers = $state<McpServerEntry[]>([]);

  // Last-saved form snapshots, diffed against current form state to build
  // each save's patch. Updated on load, reload, and after every successful
  // save (including a partial one — see `autoSave`).
  let baselineConfigFields = defaultConfigFields();
  let baselineProviderEntries: SettingsProviderEntry[] = [];
  let baselineModelAssignments = defaultModels();
  let baselineMcpServers: McpServerEntry[] = [];

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
    { id: "agent-keys", label: "Agent keys" },
    { id: "a2a", label: "A2A" },
    { id: "history", label: "History" },
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
      statusMsg = userErrorMessage(err, { action: "Couldn't load settings." });
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
    captureBaseline();
  }

  /** Snapshot current form state as the baseline the next save diffs against. */
  function captureBaseline() {
    baselineConfigFields = $state.snapshot(configFields);
    baselineProviderEntries = $state.snapshot(providerEntries);
    baselineModelAssignments = $state.snapshot(modelAssignments);
    baselineMcpServers = $state.snapshot(mcpServers);
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
      toast.error(userErrorMessage(err, { action: "Couldn't reload settings." }));
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

  // Raw mode: debounced live diagnostics as the user types, independent of
  // auto-save's own debounce so a problem shows up before the save fires.
  $effect(() => {
    if (settingsMode !== "raw") return;
    const cfg = editConfig;
    const prov = editProviders;
    const mcp = editMcp;
    const timer = setTimeout(() => {
      void validateConfig(cfg).then((r) => {
        rawDiagnostics = { ...rawDiagnostics, config: r.diagnostics ?? [] };
      });
      void validateProviders(prov).then((r) => {
        rawDiagnostics = { ...rawDiagnostics, providers: r.diagnostics ?? [] };
      });
      void validateWorkspaceFile("config/mcp.json", mcp).then((diagnostics) => {
        rawDiagnostics = { ...rawDiagnostics, mcp: diagnostics };
      });
    }, 500);
    return () => clearTimeout(timer);
  });

  async function autoSave(): Promise<void> {
    if (saving) return;
    const snap = currentSnapshot();
    if (snap === lastSavedSnapshot) return;

    saving = true;
    statusMsg = "Saving...";
    statusKind = "saving";

    try {
      if (settingsMode === "raw") {
        await autoSaveRaw();
      } else {
        await autoSaveForm();
      }
    } catch (err: unknown) {
      statusMsg = "";
      statusKind = "";
      toast.error(userErrorMessage(err, { action: "Couldn't save settings." }));
    } finally {
      saving = false;
    }
  }

  /**
   * Raw mode: PUT the whole text the user typed, unchanged from before.
   *
   * `config.toml`, `providers.toml`, and `mcp.json` all always save now,
   * even when invalid — the reload that picks each one up keeps the
   * gateway running on its current config/workspace state and reports a
   * diagnostic instead of losing the edit.
   */
  async function autoSaveRaw(): Promise<void> {
    const cfgToml = editConfig;
    const provToml = editProviders;
    const mcpJson = editMcp;

    const provResult = await putProvidersRaw(provToml);
    rawProviders = provToml;

    const cfgResult = await putConfigRaw(cfgToml);
    rawConfig = cfgToml;

    const mcpResult = await putMcpRaw(mcpJson);
    rawMcp = mcpJson;

    rawDiagnostics = {
      config: cfgResult.diagnostics ?? [],
      providers: provResult.diagnostics ?? [],
      mcp: mcpResult.diagnostics ?? [],
    };

    lastSavedSnapshot = currentSnapshot();
    const hadProblems =
      (cfgResult.diagnostics?.length ?? 0) > 0 ||
      (provResult.diagnostics?.length ?? 0) > 0 ||
      (mcpResult.diagnostics?.length ?? 0) > 0;
    showStatus(hadProblems ? "Saved — see the problems noted below" : "Saved", "success");
  }

  /**
   * Form mode: send only the diff against the last-saved baseline for each
   * file, via the server's patch endpoints — the form never rebuilds a
   * whole file from its own state (that's the bug this replaces: comments,
   * unmodeled sections/keys, and fields the form doesn't model used to be
   * silently destroyed on every save).
   *
   * Files are saved in the same order the old whole-file PUTs used
   * (providers before config, since config validation reads providers.toml
   * from disk). A failure partway through still leaves the files that
   * already saved saved — this reports exactly which files saved and which
   * didn't, and why, rather than only naming the failing one.
   */
  async function autoSaveForm(): Promise<void> {
    await storeNewSecrets();

    const currentConfig = $state.snapshot(configFields);
    const currentProviders = $state.snapshot(providerEntries);
    const currentModels = $state.snapshot(modelAssignments);
    const currentMcp = $state.snapshot(mcpServers);

    const providersDiff = diffProviders(
      baselineProviderEntries,
      currentProviders,
      baselineModelAssignments,
      currentModels,
    );
    const configDiff = diffConfigFields(baselineConfigFields, currentConfig);
    const mcpDiff = diffMcpServers(baselineMcpServers, currentMcp);

    const saved: string[] = [];
    const failed: { file: string; error: string }[] = [];

    const provResult = await patchProviders(providersDiff);
    if (provResult.valid) {
      baselineProviderEntries = currentProviders;
      baselineModelAssignments = currentModels;
      if (Object.keys(providersDiff).length > 0) saved.push("providers.toml");
    } else {
      failed.push({ file: "providers.toml", error: provResult.error ?? "unknown error" });
    }

    // config.toml validation reads providers.toml from disk, so only
    // attempt it once providers.toml is in the state config expects.
    if (provResult.valid) {
      const cfgResult = await patchConfig(configDiff);
      if (cfgResult.valid) {
        baselineConfigFields = currentConfig;
        if (Object.keys(configDiff).length > 0) saved.push("config.toml");
      } else {
        failed.push({ file: "config.toml", error: cfgResult.error ?? "unknown error" });
      }
    }

    const mcpResult = await patchMcp(mcpDiff);
    if (mcpResult.valid) {
      baselineMcpServers = currentMcp;
      if (Object.keys(mcpDiff).length > 0) saved.push("mcp.json");
    } else {
      failed.push({ file: "mcp.json", error: mcpResult.error ?? "unknown error" });
    }

    statusMsg = "";
    statusKind = "";

    if (failed.length === 0) {
      lastSavedSnapshot = currentSnapshot();
      showStatus("Saved", "success");
      return;
    }

    const failedDetail = failed.map((f) => `${f.file}: ${f.error}`).join("; ");
    const message =
      saved.length > 0
        ? `Saved ${saved.join(", ")}. Failed to save ${failedDetail}.`
        : `Failed to save ${failedDetail}.`;
    toast.error(message);
  }

  // ── Secret management ──────────────────────────────────────────────

  async function storeNewSecrets() {
    // Collect secrets that need storing: non-empty values that aren't
    // already a reference (a `secret:` lookup or a `${ENV_VAR}` expansion —
    // see isStoredReference). A `${ENV_VAR}` value must never reach
    // storeSecret: the secret store returns whatever it's given verbatim,
    // so storing the reference text would hand the provider that literal
    // placeholder as its key instead of the env var's value.
    const secretOps: {
      field:
        | "discord_token"
        | "telegram_token"
        | "teams_app_password"
        | "cloud_token"
        | "ws_brave_api_key"
        | "ws_tavily_api_key"
        | "ws_ollama_api_key";
      name: string;
    }[] = [];

    const secretFields = [
      { field: "discord_token" as const, name: "discord" },
      { field: "telegram_token" as const, name: "telegram" },
      { field: "teams_app_password" as const, name: "teams" },
      { field: "cloud_token" as const, name: "cloud_token" },
      { field: "ws_brave_api_key" as const, name: "ws_brave" },
      { field: "ws_tavily_api_key" as const, name: "ws_tavily" },
      { field: "ws_ollama_api_key" as const, name: "ws_ollama" },
    ];

    for (const { field, name } of secretFields) {
      const val = configFields[field];
      if (val && !isStoredReference(val)) {
        secretOps.push({ field, name });
      }
    }

    // Webhook secrets
    const webhookSecretOps: { idx: number; name: string }[] = [];
    for (let i = 0; i < configFields.webhooks.length; i++) {
      const wh = configFields.webhooks[i];
      if (wh?.secret && !isStoredReference(wh.secret)) {
        webhookSecretOps.push({ idx: i, name: `webhook_${wh.name}` });
      }
    }

    // Store provider API keys
    const provKeyOps: { idx: number; name: string }[] = [];
    for (let i = 0; i < providerEntries.length; i++) {
      const p = providerEntries[i];
      if (p?.apiKey && !isStoredReference(p.apiKey) && p.type !== "ollama") {
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
                onSelectSection(sec.id);
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
        {#if activeRawDiagnostics.length > 0}
          <ul class="raw-diagnostics">
            {#each activeRawDiagnostics as diagnostic, i (i)}
              <li class="raw-diagnostic raw-diagnostic-{diagnostic.severity}">
                <span class="raw-diagnostic-severity">{diagnostic.severity}</span>
                {#if diagnostic.location}
                  <span class="raw-diagnostic-location"
                    >{formatDiagnosticLocation(diagnostic.location)}</span
                  >
                {/if}
                <span class="raw-diagnostic-message">{diagnostic.message}</span>
              </li>
            {/each}
          </ul>
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
      {:else if activeSection === "agent-keys"}
        <AgentKeys />
      {:else if activeSection === "a2a"}
        <A2a bind:fields={configFields} {simple} />
      {:else if activeSection === "history"}
        <History />
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
