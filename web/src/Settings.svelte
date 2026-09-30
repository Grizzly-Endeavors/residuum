<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsMode,
    McpServerEntry,
    SettingsProviderEntry,
    SettingsModelAssignments,
    Diagnostic,
    ValidateResponse,
  } from "./lib/types";
  import {
    fetchConfigRaw,
    fetchHubConfigRaw,
    fetchProvidersRaw,
    fetchMcpRaw,
    putConfigRaw,
    putHubConfigRaw,
    putProvidersRaw,
    putMcpRaw,
    patchConfig,
    patchHubConfig,
    patchProviders,
    patchMcp,
    storeSecret,
    validateConfig,
    validateHubConfig,
    validateProviders,
    validateWorkspaceFile,
    cacheKeyConfigRaw,
    CACHE_KEY_HUB_CONFIG_RAW,
    cacheKeyProvidersRaw,
    cacheKeyMcpRaw,
  } from "./lib/api";
  import { invalidate } from "./lib/cache";
  import { isStoredReference } from "./lib/secrets";
  import { formatDiagnosticLocation } from "./lib/diagnostics";
  import { PendingSaveTracker } from "./lib/pending-save";
  import {
    parseConfigToml,
    parseProvidersToml,
    parseMcpJson,
    diffConfigFields,
    splitConfigPatch,
    diffProviders,
    diffMcpServers,
    defaultConfigFields,
    defaultModels,
    type ConfigFields,
  } from "./lib/settings-toml";
  import { sectionsFor, type SettingsScope, type SettingsSection } from "./lib/settings-sections";
  import Runtime from "./components/settings/Runtime.svelte";
  import Pulses from "./components/settings/Pulses.svelte";
  import HubGeneral from "./components/settings/HubGeneral.svelte";
  import SessionBudget from "./components/settings/SessionBudget.svelte";
  import Tracing from "./components/settings/Tracing.svelte";
  import Secrets from "./components/settings/Secrets.svelte";
  import Update from "./components/settings/Update.svelte";
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
  import { requireAgent } from "./lib/paths";

  // One page edits one scope: an agent's files, or the hub's. The page is
  // remounted when the scope or the agent changes, so `scope` is fixed for
  // the life of the component.
  let {
    scope,
    agent,
    section: activeSection,
    onSelectSection,
    onClose,
  }: {
    scope: SettingsScope;
    /** The agent whose settings these are; named in the title for agent scope. */
    agent: string | null;
    section: SettingsSection;
    onSelectSection: (section: SettingsSection) => void;
    onClose: () => void;
  } = $props();

  /**
   * The agent an agent page reads and writes; throws `NoAgentSelectedError`
   * when it has none. The page is remounted when the agent changes, so one
   * call's agent holds for the whole of a save or reload.
   */
  function scopeAgent(): string {
    return requireAgent(agent);
  }

  // ── State ──────────────────────────────────────────────────────────

  // svelte-ignore state_referenced_locally
  const isHub = scope === "hub";

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
  let rawHubConfig = $state("");
  let rawProviders = $state("");
  let rawMcp = $state("");

  // Advanced mode editing buffers
  let editConfig = $state("");
  let editHubConfig = $state("");
  let editProviders = $state("");
  let editMcp = $state("");
  let advancedTab = $state<"config" | "hub" | "providers" | "mcp">(isHub ? "hub" : "config");

  // Live diagnostics for the raw editors, refreshed on a debounce while
  // typing and replaced with each save's own diagnostics after saving.
  let rawDiagnostics = $state<{
    config: Diagnostic[];
    hub: Diagnostic[];
    providers: Diagnostic[];
    mcp: Diagnostic[];
  }>({ config: [], hub: [], providers: [], mcp: [] });
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
  // Tracks whether the debounced autosave below has (or hasn't) actually
  // sent a given change yet, so a removal made in Providers/MCP/Integrations
  // can be undone safely either way — see `lib/form-undo.ts`.
  const pendingSave = new PendingSaveTracker();

  // ── Sidebar ────────────────────────────────────────────────────────

  let mobileNavOpen = $state(false);

  // svelte-ignore state_referenced_locally
  const sections = sectionsFor(scope);

  let simple = $derived(settingsMode === "simple");

  let title = $derived.by(() => {
    if (isHub) return "Hub settings";
    return agent ? `${agent} settings` : "Settings";
  });

  let scopeNote = $derived.by(() => {
    if (isHub) return "Applies to every agent";
    return agent ? `Applies to ${agent} only` : "Applies to this agent only";
  });

  /** The Integrations file's groups that back each of the agent's sections. */
  const INTEGRATIONS_PARTS = {
    channels: "channels",
    webhooks: "webhooks",
    skills: "tools",
  } as const;

  function activeLabel(): string {
    return sections.find((s) => s.id === activeSection)?.label ?? sections[0]?.label ?? "";
  }

  // ── Load ───────────────────────────────────────────────────────────

  /**
   * Fetch the files this scope edits. An agent's page also reads the hub's
   * config, because a few of the agent's fields show hub values, but it never
   * writes it. The hub's page touches no agent files.
   */
  async function fetchScopeFiles(): Promise<void> {
    if (isHub) {
      rawHubConfig = await fetchHubConfigRaw();
      return;
    }
    const name = scopeAgent();
    const [cfgRaw, hubRaw, provRaw, mcpRaw] = await Promise.all([
      fetchConfigRaw(name),
      fetchHubConfigRaw(),
      fetchProvidersRaw(name),
      fetchMcpRaw(name),
    ]);
    rawConfig = cfgRaw;
    rawHubConfig = hubRaw;
    rawProviders = provRaw;
    rawMcp = mcpRaw;
  }

  onMount(async () => {
    try {
      await fetchScopeFiles();
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
    configFields = parseConfigToml(rawConfig, rawHubConfig);
    if (!isHub) {
      const prov = parseProvidersToml(rawProviders);
      providerEntries = prov.providers;
      modelAssignments = prov.models;
      mcpServers = parseMcpJson(rawMcp);
    }
    captureBaseline();
  }

  /** Snapshot current form state as the baseline the next save diffs against. */
  function captureBaseline() {
    baselineConfigFields = $state.snapshot(configFields);
    baselineProviderEntries = $state.snapshot(providerEntries);
    baselineModelAssignments = $state.snapshot(modelAssignments);
    baselineMcpServers = $state.snapshot(mcpServers);
  }

  /**
   * Reload just `providers.toml`/`config.toml`/`mcp.json` from disk into
   * form state, after something outside the normal save path (a
   * checkpoint restore) changed it on the server. Deliberately doesn't
   * touch `lastSavedSnapshot`: if another file still has an unsaved edit,
   * the next autosave cycle must still see it and save it. Once this
   * file's baseline matches what was just reloaded, that cycle's diff for
   * it is empty (a no-op) regardless.
   */
  async function reloadProvidersFile(): Promise<void> {
    try {
      const name = scopeAgent();
      invalidate(cacheKeyProvidersRaw(name));
      rawProviders = await fetchProvidersRaw(name);
      const prov = parseProvidersToml(rawProviders);
      providerEntries = prov.providers;
      modelAssignments = prov.models;
      baselineProviderEntries = $state.snapshot(providerEntries);
      baselineModelAssignments = $state.snapshot(modelAssignments);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload providers.toml." }));
    }
  }

  async function reloadConfigFile(): Promise<void> {
    try {
      const name = scopeAgent();
      invalidate(cacheKeyConfigRaw(name));
      invalidate(CACHE_KEY_HUB_CONFIG_RAW);
      [rawConfig, rawHubConfig] = await Promise.all([fetchConfigRaw(name), fetchHubConfigRaw()]);
      configFields = parseConfigToml(rawConfig, rawHubConfig);
      baselineConfigFields = $state.snapshot(configFields);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload config.toml." }));
    }
  }

  async function reloadMcpFile(): Promise<void> {
    try {
      const name = scopeAgent();
      invalidate(cacheKeyMcpRaw(name));
      rawMcp = await fetchMcpRaw(name);
      mcpServers = parseMcpJson(rawMcp);
      baselineMcpServers = $state.snapshot(mcpServers);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload mcp.json." }));
    }
  }

  // ── Mode switching ────────────────────────────────────────────────

  function setMode(mode: SettingsMode) {
    if (mode === settingsMode) return;
    statusMsg = "";
    statusKind = "";

    if (mode === "raw") {
      // Entering raw — load text editors
      editConfig = rawConfig;
      editHubConfig = rawHubConfig;
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
      await fetchScopeFiles();

      if (settingsMode === "raw") {
        editConfig = rawConfig;
        editHubConfig = rawHubConfig;
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
      return `adv:${editConfig}|${editHubConfig}|${editProviders}|${editMcp}`;
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
    pendingSave.markScheduled(autoSaveTimer);
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
    editHubConfig;
    editProviders;
    editMcp;
    if (settingsMode === "raw") scheduleAutoSave();
  });

  // Raw mode: debounced live diagnostics as the user types, independent of
  // auto-save's own debounce so a problem shows up before the save fires.
  $effect(() => {
    if (settingsMode !== "raw") return;
    const cfg = editConfig;
    const hub = editHubConfig;
    const prov = editProviders;
    const mcp = editMcp;
    const timer = setTimeout(() => {
      if (isHub) {
        void validateHubConfig(hub).then((r) => {
          rawDiagnostics = { ...rawDiagnostics, hub: r.diagnostics ?? [] };
        });
        return;
      }
      // An agent page with no agent has nothing to validate against.
      if (agent === null) return;
      void validateConfig(agent, cfg).then((r) => {
        rawDiagnostics = { ...rawDiagnostics, config: r.diagnostics ?? [] };
      });
      void validateProviders(agent, prov).then((r) => {
        rawDiagnostics = { ...rawDiagnostics, providers: r.diagnostics ?? [] };
      });
      void validateWorkspaceFile(agent, "config/mcp.json", mcp).then((diagnostics) => {
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
    pendingSave.markSaving();
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
      pendingSave.markSettled();
    }
  }

  /**
   * Raw mode: PUT the whole text the user typed, unchanged from before.
   *
   * The hub's `config.toml`, the agent's `config.toml` and `providers.toml`,
   * and `mcp.json` all always save, even when invalid — the reload that picks each one up keeps the
   * gateway running on its current config/workspace state and reports a
   * diagnostic instead of losing the edit.
   */
  async function autoSaveRaw(): Promise<void> {
    const cfgToml = editConfig;
    const hubToml = editHubConfig;
    const provToml = editProviders;
    const mcpJson = editMcp;

    if (isHub) {
      const hubResult = await putHubConfigRaw(hubToml);
      rawHubConfig = hubToml;
      rawDiagnostics = { ...rawDiagnostics, hub: hubResult.diagnostics ?? [] };
      lastSavedSnapshot = currentSnapshot();
      showStatus(
        (hubResult.diagnostics?.length ?? 0) > 0 ? "Saved — see the problems noted below" : "Saved",
        "success",
      );
      return;
    }

    // The agent's page never rewrites the hub's config.
    const name = scopeAgent();
    const provResult = await putProvidersRaw(name, provToml);
    rawProviders = provToml;

    const cfgResult = await putConfigRaw(name, cfgToml);
    rawConfig = cfgToml;

    const mcpResult = await putMcpRaw(name, mcpJson);
    rawMcp = mcpJson;

    rawDiagnostics = {
      config: cfgResult.diagnostics ?? [],
      hub: rawDiagnostics.hub,
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

  /** Form mode on the hub's page: patch the hub's `config.toml` with what changed. */
  async function saveHubForm(
    hubDiff: Record<string, unknown>,
    currentConfig: ConfigFields,
  ): Promise<void> {
    const changed = Object.keys(hubDiff).length > 0;
    const result: ValidateResponse = changed ? await patchHubConfig(hubDiff) : { valid: true };
    statusMsg = "";
    statusKind = "";
    if (!result.valid) {
      toast.error(`Failed to save hub config.toml: ${result.error ?? "unknown error"}.`);
      return;
    }
    baselineConfigFields = currentConfig;
    if (changed) pendingSave.recordWrite("hub/config.toml", result.checkpoint_id ?? null);
    lastSavedSnapshot = currentSnapshot();
    showStatus("Saved", "success");
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
    const { hub: hubDiff, agent: configDiff } = splitConfigPatch(
      diffConfigFields(baselineConfigFields, currentConfig),
    );
    const mcpDiff = diffMcpServers(baselineMcpServers, currentMcp);

    if (isHub) {
      await saveHubForm(hubDiff, currentConfig);
      return;
    }

    const saved: string[] = [];
    const failed: { file: string; error: string }[] = [];

    const name = scopeAgent();
    const provResult = await patchProviders(name, providersDiff);
    if (provResult.valid) {
      baselineProviderEntries = currentProviders;
      baselineModelAssignments = currentModels;
      if (Object.keys(providersDiff).length > 0) {
        saved.push("providers.toml");
        pendingSave.recordWrite("providers.toml", provResult.checkpoint_id ?? null);
      }
    } else {
      failed.push({ file: "providers.toml", error: provResult.error ?? "unknown error" });
    }

    // config.toml validation reads providers.toml from disk, so only
    // attempt it once providers.toml is in the state config expects.
    if (provResult.valid) {
      const cfgResult = await patchConfig(name, configDiff);
      if (cfgResult.valid) {
        baselineConfigFields = currentConfig;
        if (Object.keys(configDiff).length > 0) {
          saved.push("config.toml");
          pendingSave.recordWrite("config.toml", cfgResult.checkpoint_id ?? null);
        }
      } else {
        failed.push({ file: "config.toml", error: cfgResult.error ?? "unknown error" });
      }
    }

    const mcpResult = await patchMcp(name, mcpDiff);
    if (mcpResult.valid) {
      baselineMcpServers = currentMcp;
      if (Object.keys(mcpDiff).length > 0) {
        saved.push("mcp.json");
        pendingSave.recordWrite("config/mcp.json", mcpResult.checkpoint_id ?? null);
      }
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
      // The hub's page owns the cloud token; an agent's page owns the rest.
      if ((field === "cloud_token") !== isHub) continue;
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
    <div class="settings-heading">
      <h2 class="settings-title">{title}</h2>
      <span class="settings-scope">{scopeNote}</span>
    </div>
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
      <div class="settings-mode-selector" role="group" aria-label="Settings view">
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "simple"}
          aria-pressed={settingsMode === "simple"}
          onclick={() => setMode("simple")}
        >
          Simple
        </button>
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "advanced"}
          aria-pressed={settingsMode === "advanced"}
          onclick={() => setMode("advanced")}
        >
          Advanced
        </button>
        <button
          class="settings-mode-btn"
          class:active={settingsMode === "raw"}
          aria-pressed={settingsMode === "raw"}
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
      <nav
        class="settings-sidebar"
        class:collapsed={!mobileNavOpen}
        aria-label={isHub ? "Hub settings sections" : "Settings sections"}
      >
        <button
          class="settings-nav-toggle"
          aria-expanded={mobileNavOpen}
          aria-controls="settings-nav-items"
          onclick={() => {
            mobileNavOpen = !mobileNavOpen;
          }}
        >
          <span>{activeLabel()}</span>
          <span class="nav-chevron" class:open={mobileNavOpen}>&#9660;</span>
        </button>
        <div class="settings-nav-items" id="settings-nav-items">
          {#each sections as sec (sec.id)}
            <button
              class="settings-sidebar-btn"
              class:active={activeSection === sec.id}
              aria-current={activeSection === sec.id ? "page" : undefined}
              onclick={() => {
                onSelectSection(sec.id);
                mobileNavOpen = false;
              }}
            >
              {sec.label}
            </button>
          {/each}
        </div>
      </nav>
    {/if}

    <div class="settings-content">
      {#if loading}
        <p style="color:var(--text-dim); padding:20px;">Loading settings...</p>
      {:else if settingsMode === "raw"}
        <!-- Raw tabbed editor -->
        {#if !isHub}
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
        {:else}
          <div class="advanced-tabs">
            <span class="advanced-tab active">hub config.toml</span>
          </div>
        {/if}
        {#if advancedTab === "config"}
          <textarea class="toml-editor" bind:value={editConfig}></textarea>
        {:else if advancedTab === "hub"}
          <textarea class="toml-editor" bind:value={editHubConfig}></textarea>
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
      {:else if isHub}
        {#if activeSection === "general"}
          <HubGeneral bind:fields={configFields} {simple} />
        {:else if activeSection === "cloud"}
          <Integrations
            bind:fields={configFields}
            {simple}
            part="cloud"
            {agent}
            {pendingSave}
            onReload={reloadConfigFile}
          />
        {:else if activeSection === "a2a"}
          <A2a bind:fields={configFields} {simple} scope="hub" {agent} />
        {:else if activeSection === "sessions"}
          <SessionBudget bind:fields={configFields} />
        {:else if activeSection === "tracing"}
          <Tracing bind:fields={configFields} />
        {:else if activeSection === "update"}
          <Update />
        {:else if activeSection === "secrets"}
          <Secrets />
        {:else if activeSection === "agent-keys"}
          <AgentKeys />
        {:else if activeSection === "history"}
          <History scope="hub" {agent} />
        {/if}
      {:else if activeSection === "runtime"}
        <Runtime bind:fields={configFields} {simple} />
      {:else if activeSection === "providers"}
        <Providers
          bind:providers={providerEntries}
          bind:models={modelAssignments}
          {agent}
          {pendingSave}
          onReload={reloadProvidersFile}
        />
      {:else if activeSection === "channels" || activeSection === "skills" || activeSection === "webhooks"}
        <Integrations
          bind:fields={configFields}
          {simple}
          part={INTEGRATIONS_PARTS[activeSection]}
          {agent}
          {pendingSave}
          onReload={reloadConfigFile}
        />
      {:else if activeSection === "pulses"}
        <Pulses bind:fields={configFields} />
      {:else if activeSection === "memory"}
        <Memory bind:fields={configFields} {simple} />
      {:else if activeSection === "mcp"}
        <MCP bind:servers={mcpServers} {agent} {pendingSave} onReload={reloadMcpFile} />
      {:else if activeSection === "a2a"}
        <A2a bind:fields={configFields} {simple} scope="agent" {agent} />
      {:else if activeSection === "history"}
        <History scope="agent" {agent} />
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
