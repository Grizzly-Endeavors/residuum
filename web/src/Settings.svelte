<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsMode,
    McpServerEntry,
    SettingsProviderEntry,
    SettingsModelAssignments,
    Diagnostic,
  } from "./lib/types";
  import {
    fetchConfigRaw,
    fetchHubConfigRaw,
    fetchProvidersRaw,
    fetchMcpRaw,
    storeSecret,
    validateConfig,
    validateHubConfig,
    validateProviders,
    validateWorkspaceFile,
  } from "./lib/api";
  import {
    agentConfigFile,
    configCoordinator,
    configFileName,
    HUB_CONFIG_FILE,
    type ConfigChange,
    type ConfigChoice,
    type ConfigConflict,
    type ConfigEdit,
    type ConfigFile,
    type ConfigSaved,
  } from "./lib/config-coordinator";
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
  import {
    legacySectionsFor,
    type LegacyScope,
    type LegacySection,
  } from "./lib/legacy-settings-sections";
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
    scope: LegacyScope;
    /** The agent whose settings these are; named in the title for agent scope. */
    agent: string | null;
    section: LegacySection;
    onSelectSection: (section: LegacySection) => void;
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
  const sections = legacySectionsFor(scope);

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

  /** Names this page in the coordinator's notifications, so it can skip the changes it made. */
  const SELF = Symbol("settings");

  /**
   * Fetch the files this scope edits. An agent's page also reads the hub's
   * config, because a few of the agent's fields show hub values, but it never
   * writes it. The hub's page touches no agent files. `fresh` reads from disk
   * and tells every other view of these files to do the same; without it a
   * cached copy will do.
   */
  async function fetchScopeFiles(fresh: boolean): Promise<void> {
    const read = (file: ConfigFile, cached: () => Promise<string>): Promise<string> =>
      fresh ? configCoordinator.reload(file, SELF) : cached();
    if (isHub) {
      rawHubConfig = await read(HUB_CONFIG_FILE, fetchHubConfigRaw);
      return;
    }
    const name = scopeAgent();
    const [cfgRaw, hubRaw, provRaw, mcpRaw] = await Promise.all([
      read(agentConfigFile(name, "config"), () => fetchConfigRaw(name)),
      read(HUB_CONFIG_FILE, fetchHubConfigRaw),
      read(agentConfigFile(name, "providers"), () => fetchProvidersRaw(name)),
      read(agentConfigFile(name, "mcp"), () => fetchMcpRaw(name)),
    ]);
    rawConfig = cfgRaw;
    rawHubConfig = hubRaw;
    rawProviders = provRaw;
    rawMcp = mcpRaw;
  }

  onMount(() => {
    void loadInitial();
    return followFileChanges();
  });

  async function loadInitial(): Promise<void> {
    try {
      await fetchScopeFiles(false);
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
  }

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

  // ── Following changes made elsewhere ───────────────────────────────
  //
  // The coordinator reads a file from disk before it tells anyone it
  // changed, so the fetches below find the current text in the cache.
  //
  // Each reload puts a file's current text into the form, or into its raw
  // editor. It deliberately doesn't touch `lastSavedSnapshot`: if another
  // file still has an unsaved edit, the next autosave cycle must still see it
  // and save it. Once this file's baseline matches what was just reloaded,
  // that cycle's diff for it is empty (a no-op) regardless.

  async function reloadProvidersFile(): Promise<void> {
    try {
      rawProviders = await fetchProvidersRaw(scopeAgent());
      if (settingsMode === "raw") {
        editProviders = rawProviders;
        return;
      }
      const prov = parseProvidersToml(rawProviders);
      providerEntries = prov.providers;
      modelAssignments = prov.models;
      baselineProviderEntries = $state.snapshot(providerEntries);
      baselineModelAssignments = $state.snapshot(modelAssignments);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload providers.toml." }));
    }
  }

  /** The agent's `config.toml` and the hub's together, since the form shows both. */
  async function reloadConfigFile(): Promise<void> {
    try {
      if (isHub) {
        rawHubConfig = await fetchHubConfigRaw();
      } else {
        const name = scopeAgent();
        [rawConfig, rawHubConfig] = await Promise.all([fetchConfigRaw(name), fetchHubConfigRaw()]);
      }
      if (settingsMode === "raw") {
        editConfig = rawConfig;
        editHubConfig = rawHubConfig;
        return;
      }
      configFields = parseConfigToml(rawConfig, rawHubConfig);
      baselineConfigFields = $state.snapshot(configFields);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload config.toml." }));
    }
  }

  async function reloadMcpFile(): Promise<void> {
    try {
      rawMcp = await fetchMcpRaw(scopeAgent());
      if (settingsMode === "raw") {
        editMcp = rawMcp;
        return;
      }
      mcpServers = parseMcpJson(rawMcp);
      baselineMcpServers = $state.snapshot(mcpServers);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't reload mcp.json." }));
    }
  }

  /**
   * Whether the page holds edits to a file that no save has written: what the
   * next save would send for it. (The panels also fill in empty values the
   * file doesn't have, which isn't an edit.)
   */
  function hasUnsavedEdits(file: "config" | "providers" | "mcp"): boolean {
    if (settingsMode === "raw") {
      if (file === "config") return editConfig !== rawConfig || editHubConfig !== rawHubConfig;
      return file === "providers" ? editProviders !== rawProviders : editMcp !== rawMcp;
    }
    if (file === "config") {
      return (
        Object.keys(diffConfigFields(baselineConfigFields, $state.snapshot(configFields))).length >
        0
      );
    }
    if (file === "providers") {
      const changes = diffProviders(
        baselineProviderEntries,
        $state.snapshot(providerEntries),
        baselineModelAssignments,
        $state.snapshot(modelAssignments),
      );
      return Object.keys(changes).length > 0;
    }
    return Object.keys(diffMcpServers(baselineMcpServers, $state.snapshot(mcpServers))).length > 0;
  }

  /**
   * Listen for one file changing. A restore is the user's own request, so it
   * replaces what the page shows. Any other change waits while the page holds
   * unsaved edits to that file, so typing isn't lost; the coordinator checks
   * those edits against the file when they are saved.
   */
  function whenChanged(
    file: "config" | "providers" | "mcp",
    reload: () => Promise<void>,
  ): (change: ConfigChange) => void {
    return (change) => {
      if (change.source === SELF || !initialized) return;
      if (change.cause !== "restore" && hasUnsavedEdits(file)) return;
      void reload();
    };
  }

  /** Follow the files this scope shows. Returns a function that stops. */
  function followFileChanges(): () => void {
    if (isHub) {
      return configCoordinator.subscribe(HUB_CONFIG_FILE, whenChanged("config", reloadConfigFile));
    }
    if (agent === null) return () => {};
    const stops = [
      configCoordinator.subscribe(
        agentConfigFile(agent, "config"),
        whenChanged("config", reloadConfigFile),
      ),
      // The agent's form shows a few hub values too.
      configCoordinator.subscribe(HUB_CONFIG_FILE, whenChanged("config", reloadConfigFile)),
      configCoordinator.subscribe(
        agentConfigFile(agent, "providers"),
        whenChanged("providers", reloadProvidersFile),
      ),
      configCoordinator.subscribe(agentConfigFile(agent, "mcp"), whenChanged("mcp", reloadMcpFile)),
    ];
    return () => {
      for (const stop of stops) stop();
    };
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
      await fetchScopeFiles(true);

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

  // ── Saving through the coordinator ────────────────────────────────

  /** Names a file in the page's messages. */
  function describeFile(file: ConfigFile): string {
    return file.kind === "hub" ? "the hub's config.toml" : configFileName(file);
  }

  /**
   * This page keeps its own changes when a file changed on disk under them,
   * and says what it replaced.
   */
  function keepMine(conflict: ConfigConflict): Promise<ConfigChoice> {
    const where = conflict.keys.length > 0 ? ` to ${conflict.keys.join(", ")}` : "";
    toast.info(`Saving replaced changes made elsewhere${where} in ${describeFile(conflict.file)}.`);
    return Promise.resolve("keep-mine");
  }

  /** Save `edit` to a file through the coordinator, against the text this page last loaded or saved. */
  async function saveFile(
    file: ConfigFile,
    baseline: string,
    edit: ConfigEdit,
  ): Promise<ConfigSaved> {
    const outcome = await configCoordinator.save(file, {
      baseline,
      edit,
      choose: keepMine,
      source: SELF,
    });
    // `keepMine` never asks for the file on disk, so the coordinator always writes.
    if (outcome.kind === "used-disk") {
      throw new Error(`${describeFile(file)} was left as it is on disk instead of being saved`);
    }
    return outcome;
  }

  /**
   * Raw mode: PUT the whole text the user typed, for each file they edited.
   *
   * The hub's `config.toml`, the agent's `config.toml` and `providers.toml`,
   * and `mcp.json` all always save, even when invalid — the reload that picks each one up keeps the
   * gateway running on its current config/workspace state and reports a
   * diagnostic instead of losing the edit. A file the user didn't edit is
   * left alone: writing this page's older copy of it would put it over any
   * change made to that file elsewhere.
   */
  async function autoSaveRaw(): Promise<void> {
    const diagnostics = { ...rawDiagnostics };

    if (isHub) {
      const hubToml = editHubConfig;
      if (hubToml !== rawHubConfig) {
        const saved = await saveFile(HUB_CONFIG_FILE, rawHubConfig, { text: hubToml });
        rawHubConfig = hubToml;
        diagnostics.hub = saved.result.diagnostics ?? [];
      }
    } else {
      // The agent's page never rewrites the hub's config.
      const name = scopeAgent();
      const provToml = editProviders;
      if (provToml !== rawProviders) {
        const saved = await saveFile(agentConfigFile(name, "providers"), rawProviders, {
          text: provToml,
        });
        rawProviders = provToml;
        diagnostics.providers = saved.result.diagnostics ?? [];
      }
      const cfgToml = editConfig;
      if (cfgToml !== rawConfig) {
        const saved = await saveFile(agentConfigFile(name, "config"), rawConfig, {
          text: cfgToml,
        });
        rawConfig = cfgToml;
        diagnostics.config = saved.result.diagnostics ?? [];
      }
      const mcpJson = editMcp;
      if (mcpJson !== rawMcp) {
        const saved = await saveFile(agentConfigFile(name, "mcp"), rawMcp, { text: mcpJson });
        rawMcp = mcpJson;
        diagnostics.mcp = saved.result.diagnostics ?? [];
      }
    }

    rawDiagnostics = diagnostics;
    lastSavedSnapshot = currentSnapshot();
    const hadProblems = Object.values(diagnostics).some((list) => list.length > 0);
    showStatus(hadProblems ? "Saved — see the problems noted below" : "Saved", "success");
  }

  /** Form mode on the hub's page: patch the hub's `config.toml` with what changed. */
  async function saveHubForm(
    hubDiff: Record<string, unknown>,
    currentConfig: ConfigFields,
  ): Promise<void> {
    const saved = await saveFile(HUB_CONFIG_FILE, rawHubConfig, { patch: hubDiff });
    statusMsg = "";
    statusKind = "";
    if (!saved.result.valid) {
      toast.error(`Failed to save hub config.toml: ${saved.result.error ?? "unknown error"}.`);
      return;
    }
    baselineConfigFields = currentConfig;
    if (saved.raw !== null) rawHubConfig = saved.raw;
    if (saved.written)
      pendingSave.recordWrite("hub/config.toml", saved.result.checkpoint_id ?? null);
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

    /**
     * Patch one file. `true` when the file now has what the form holds;
     * otherwise the reason is in `failed`. `recorded` is the file's name in
     * the pending-save tracker.
     */
    const patchFile = async (
      file: ConfigFile,
      baseline: string,
      patch: Record<string, unknown>,
      recorded: string,
      setRaw: (raw: string) => void,
    ): Promise<boolean> => {
      const label = configFileName(file);
      const result = await saveFile(file, baseline, { patch });
      if (!result.result.valid) {
        failed.push({ file: label, error: result.result.error ?? "unknown error" });
        return false;
      }
      if (result.raw !== null) setRaw(result.raw);
      if (result.written) {
        saved.push(label);
        pendingSave.recordWrite(recorded, result.result.checkpoint_id ?? null);
      }
      return true;
    };

    const name = scopeAgent();
    const providersSaved = await patchFile(
      agentConfigFile(name, "providers"),
      rawProviders,
      providersDiff,
      "providers.toml",
      (raw) => {
        rawProviders = raw;
      },
    );
    if (providersSaved) {
      baselineProviderEntries = currentProviders;
      baselineModelAssignments = currentModels;
    }

    // config.toml validation reads providers.toml from disk, so only
    // attempt it once providers.toml is in the state config expects.
    if (providersSaved) {
      const configSaved = await patchFile(
        agentConfigFile(name, "config"),
        rawConfig,
        configDiff,
        "config.toml",
        (raw) => {
          rawConfig = raw;
        },
      );
      if (configSaved) baselineConfigFields = currentConfig;
    }

    const mcpSaved = await patchFile(
      agentConfigFile(name, "mcp"),
      rawMcp,
      mcpDiff,
      "config/mcp.json",
      (raw) => {
        rawMcp = raw;
      },
    );
    if (mcpSaved) baselineMcpServers = currentMcp;

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
