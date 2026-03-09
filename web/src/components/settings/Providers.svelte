<script lang="ts">
  import { onMount } from "svelte";
  import type {
    SettingsProviderEntry,
    SettingsModelAssignments,
    ModelRoleKey,
  } from "../../lib/types";
  import {
    fetchModels,
    DEFAULT_MODELS,
    DEFAULT_EMBEDDING_MODELS,
    EMBEDDING_PROVIDERS,
    EMBEDDING_MODEL_LISTS,
    debounce,
    invalidateProvider,
    type ModelEntry,
  } from "../../lib/models";

  let {
    providers = $bindable(),
    models = $bindable(),
  }: {
    providers: SettingsProviderEntry[];
    models: SettingsModelAssignments;
  } = $props();

  const providerTypes: Record<string, string> = {
    anthropic: "Anthropic",
    openai: "OpenAI",
    gemini: "Google Gemini",
    ollama: "Ollama",
  };

  const roleTooltips: Record<string, string> = {
    main: "The primary model used for conversations and task execution.",
    observer:
      "Watches conversations and extracts facts, preferences, and patterns for long-term memory.",
    reflector:
      "Periodically reviews stored memories, consolidates duplicates, and resolves contradictions.",
    pulse: "Drives proactive behavior — daily briefings, check-ins, and ambient monitoring tasks.",
    embedding:
      "Generates vector embeddings for semantic memory search. Only some providers support this.",
    "bg-small":
      "Used for lightweight background tasks like formatting, simple lookups, and notifications.",
    "bg-medium": "Used for moderate background tasks like summarization and analysis.",
    "bg-large": "Used for complex background tasks that need strong reasoning ability.",
  };

  const allRoles = ["main", "observer", "reflector", "pulse"];
  const bgTiers = ["bg-small", "bg-medium", "bg-large"];

  const thinkingOptions = [
    { value: "", label: "Default" },
    { value: "off", label: "Off" },
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
  ];

  /** Ensure the overrides record has an entry for a given key. */
  function ensureOverrides(key: string) {
    if (!models.overrides[key]) {
      models.overrides[key] = { temperature: "", thinking: "" };
      models.overrides = models.overrides;
    }
  }

  // Model lists and loading state per role
  let modelLists = $state<Record<string, ModelEntry[]>>({});
  let modelLoading = $state<Record<string, boolean>>({});
  let otherActive = $state<Record<string, boolean>>({});
  let otherValues = $state<Record<string, string>>({});

  // Map role key to models field key
  function modelsKey(role: string): ModelRoleKey {
    if (role === "bg-small") return "bgSmall";
    if (role === "bg-medium") return "bgMedium";
    if (role === "bg-large") return "bgLarge";
    return role as ModelRoleKey;
  }

  // Extract provider from "provider/model" string
  function getProvider(role: string): string {
    const val = models[modelsKey(role)];
    if (!val) return providers[0]?.name ?? "";
    const slash = val.indexOf("/");
    return slash >= 0 ? val.slice(0, slash) : (providers[0]?.name ?? "");
  }

  // Extract model from "provider/model" string
  function getModel(role: string): string {
    const val = models[modelsKey(role)];
    if (!val) return "";
    const slash = val.indexOf("/");
    return slash >= 0 ? val.slice(slash + 1) : val;
  }

  function setProvider(role: string, prov: string) {
    const key = modelsKey(role);
    models[key] = prov + "/";
    otherActive[role] = false;
    otherValues[role] = "";
    void loadModels(role);
  }

  function setModel(role: string, value: string) {
    if (value === "__other__") {
      otherActive[role] = true;
      return;
    }
    otherActive[role] = false;
    const prov = getProvider(role);
    models[modelsKey(role)] = prov + "/" + value;
  }

  function setOtherModel(role: string, value: string) {
    otherValues[role] = value;
    const prov = getProvider(role);
    models[modelsKey(role)] = prov + "/" + value;
  }

  // Find provider entry for a role's current provider name
  function providerEntry(name: string): SettingsProviderEntry | undefined {
    return providers.find((p) => p.name === name);
  }

  async function loadModels(role: string) {
    const provName = getProvider(role);
    const entry = providerEntry(provName);
    if (!entry) return;

    // Embedding uses hardcoded lists
    if (role === "embedding") {
      const list = EMBEDDING_MODEL_LISTS[entry.type] ?? [];
      modelLists[role] = list;
      const current = getModel(role);
      if (!current && list.length > 0) {
        const def = DEFAULT_EMBEDDING_MODELS[entry.type] ?? "";
        const found = list.some((m) => m.id === def);
        setModel(role, found ? def : (list[0]?.id ?? ""));
      }
      return;
    }

    const apiKey = entry.apiKey || undefined;
    const url = entry.url !== "" ? entry.url : undefined;

    modelLoading[role] = true;
    const result = await fetchModels(entry.type, apiKey, url);
    modelLists[role] = result.models;
    modelLoading[role] = false;

    const current = getModel(role);
    if (!current && result.models.length > 0) {
      const def = DEFAULT_MODELS[entry.type] ?? "";
      const found = result.models.some((m) => m.id === def);
      setModel(role, found ? def : (result.models[0]?.id ?? ""));
    }
  }

  const _debouncedLoadModels = debounce((role: string) => {
    void loadModels(role);
  }, 500);

  function addProvider() {
    providers.push({ name: "", type: "anthropic", apiKey: "", url: "", keepAlive: "" });
    providers = providers;
  }

  function removeProvider(idx: number) {
    providers.splice(idx, 1);
    providers = providers;
  }

  function providerNameOptions(): string[] {
    return providers.map((p) => p.name).filter(Boolean);
  }

  function roleLabel(role: string): string {
    const labels: Record<string, string> = {
      main: "Main Agent",
      observer: "Observer",
      reflector: "Reflector",
      pulse: "Pulse",
      embedding: "Embedding",
      "bg-small": "Small",
      "bg-medium": "Medium",
      "bg-large": "Large",
    };
    return labels[role] ?? role;
  }

  onMount(() => {
    if (providers.length === 0) return;
    for (const role of [...allRoles, "embedding", ...bgTiers]) {
      void loadModels(role);
    }
  });
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Provider Connections</div>

    {#each providers as prov, i (i)}
      <div class="provider-entry">
        <div class="provider-entry-fields">
          <div class="settings-field">
            <label for="settings-prov-{i}-name">Name</label>
            <input
              id="settings-prov-{i}-name"
              type="text"
              bind:value={prov.name}
              placeholder="e.g. anthropic"
            />
          </div>
          <div class="settings-field">
            <label for="settings-prov-{i}-type">Type</label>
            <select id="settings-prov-{i}-type" bind:value={prov.type}>
              {#each Object.entries(providerTypes) as [val, label] (val)}
                <option value={val}>{label}</option>
              {/each}
            </select>
          </div>
          <div class="settings-field">
            <label for="settings-prov-{i}-apikey"
              >API Key{#if prov.type === "ollama"}
                (optional){/if}</label
            >
            {#if prov.apiKey.startsWith("secret:")}
              <div class="secret-stored">
                <span class="secret-badge">Stored securely</span>
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    prov.apiKey = "";
                  }}>Change</button
                >
              </div>
            {:else}
              <input
                id="settings-prov-{i}-apikey"
                type="password"
                bind:value={prov.apiKey}
                placeholder="API key"
                onblur={() => {
                  invalidateProvider(prov.type);
                  for (const role of [...allRoles, "embedding", ...bgTiers]) {
                    if (getProvider(role) === prov.name) _debouncedLoadModels(role);
                  }
                }}
              />
            {/if}
          </div>
          <div class="settings-field">
            <label for="settings-prov-{i}-url">Base URL (optional)</label>
            <input
              id="settings-prov-{i}-url"
              type="text"
              bind:value={prov.url}
              placeholder="Override base URL"
            />
          </div>
          {#if prov.type === "ollama"}
            <div class="settings-field">
              <label for="settings-prov-{i}-keepalive">Keep Alive</label>
              <input
                id="settings-prov-{i}-keepalive"
                type="text"
                bind:value={prov.keepAlive}
                placeholder="5m (default)"
              />
            </div>
          {/if}
        </div>
        <button class="btn btn-sm btn-danger" onclick={() => removeProvider(i)}>Remove</button>
      </div>
    {/each}

    <button class="btn btn-secondary btn-sm" onclick={addProvider}>+ Add Provider</button>
  </div>

  <!-- Model Role Assignments -->
  <div class="settings-group">
    <div class="settings-group-label">Model Role Assignments</div>

    <div class="roles-section">
      <div class="roles-section-label">Agent</div>
      {#each ["main"] as role (role)}
        {@const ovKey = modelsKey(role)}
        <div class="role-row">
          <div class="role-row-label">
            {roleLabel(role)}
            <span class="role-tooltip">
              <span class="role-tooltip-icon">?</span>
              <span class="role-tooltip-text">{roleTooltips[role]}</span>
            </span>
          </div>
          <div class="role-row-fields">
            <div class="settings-field">
              <label for="srole-{role}-provider">Provider</label>
              <select
                id="srole-{role}-provider"
                value={getProvider(role)}
                onchange={(e) => setProvider(role, (e.target as HTMLSelectElement).value)}
              >
                {#each providerNameOptions() as pk (pk)}
                  <option value={pk}>{pk}</option>
                {/each}
              </select>
            </div>
            <div class="settings-field">
              <label for="srole-{role}-model">Model</label>
              <div class="model-select-wrap">
                <select
                  id="srole-{role}-model"
                  value={otherActive[role] ? "__other__" : getModel(role)}
                  onchange={(e) => setModel(role, (e.target as HTMLSelectElement).value)}
                  disabled={modelLoading[role]}
                >
                  {#if modelLoading[role]}
                    <option value="">Loading...</option>
                  {:else}
                    {#each modelLists[role] ?? [] as m (m.id)}
                      <option value={m.id}>{m.name ?? m.id}</option>
                    {/each}
                    <option value="__other__">Other...</option>
                  {/if}
                </select>
                {#if otherActive[role]}
                  <input
                    class="model-other-input"
                    type="text"
                    placeholder="Enter model ID..."
                    value={otherValues[role] ?? ""}
                    oninput={(e) => setOtherModel(role, (e.target as HTMLInputElement).value)}
                  />
                {/if}
              </div>
            </div>
          </div>
          <div class="role-overrides">
            <div class="settings-field override-field">
              <label for="srole-{role}-temp">Temperature</label>
              <input
                id="srole-{role}-temp"
                type="number"
                step="0.1"
                min="0"
                max="2"
                value={models.overrides[ovKey]?.temperature ?? ""}
                oninput={(e) => {
                  ensureOverrides(ovKey);
                  const ov = models.overrides[ovKey];
                  if (ov) ov.temperature = (e.target as HTMLInputElement).value;
                }}
                placeholder="Global default"
              />
            </div>
            <div class="settings-field override-field">
              <span class="override-label">Thinking</span>
              <div class="segmented-control">
                {#each thinkingOptions as opt (opt.value)}
                  <button
                    type="button"
                    class="seg-btn"
                    class:active={(models.overrides[ovKey]?.thinking ?? "") === opt.value}
                    onclick={() => {
                      ensureOverrides(ovKey);
                      const ov = models.overrides[ovKey];
                      if (ov) ov.thinking = opt.value;
                      models.overrides = models.overrides;
                    }}
                  >
                    {opt.label}
                  </button>
                {/each}
              </div>
            </div>
          </div>
        </div>
      {/each}
    </div>

    <div class="roles-section">
      <div class="roles-section-label">Subsystems</div>
      {#each ["observer", "reflector", "pulse"] as role (role)}
        {@const ovKey = modelsKey(role)}
        <div class="role-row">
          <div class="role-row-label">
            {roleLabel(role)}
            <span class="role-tooltip">
              <span class="role-tooltip-icon">?</span>
              <span class="role-tooltip-text">{roleTooltips[role]}</span>
            </span>
          </div>
          <div class="role-row-fields">
            <div class="settings-field">
              <label for="srole-{role}-provider">Provider</label>
              <select
                id="srole-{role}-provider"
                value={getProvider(role)}
                onchange={(e) => setProvider(role, (e.target as HTMLSelectElement).value)}
              >
                {#each providerNameOptions() as pk (pk)}
                  <option value={pk}>{pk}</option>
                {/each}
              </select>
            </div>
            <div class="settings-field">
              <label for="srole-{role}-model">Model</label>
              <div class="model-select-wrap">
                <select
                  id="srole-{role}-model"
                  value={otherActive[role] ? "__other__" : getModel(role)}
                  onchange={(e) => setModel(role, (e.target as HTMLSelectElement).value)}
                  disabled={modelLoading[role]}
                >
                  {#if modelLoading[role]}
                    <option value="">Loading...</option>
                  {:else}
                    {#each modelLists[role] ?? [] as m (m.id)}
                      <option value={m.id}>{m.name ?? m.id}</option>
                    {/each}
                    <option value="__other__">Other...</option>
                  {/if}
                </select>
                {#if otherActive[role]}
                  <input
                    class="model-other-input"
                    type="text"
                    placeholder="Enter model ID..."
                    value={otherValues[role] ?? ""}
                    oninput={(e) => setOtherModel(role, (e.target as HTMLInputElement).value)}
                  />
                {/if}
              </div>
            </div>
          </div>
          <div class="role-overrides">
            <div class="settings-field override-field">
              <label for="srole-{role}-temp">Temperature</label>
              <input
                id="srole-{role}-temp"
                type="number"
                step="0.1"
                min="0"
                max="2"
                value={models.overrides[ovKey]?.temperature ?? ""}
                oninput={(e) => {
                  ensureOverrides(ovKey);
                  const ov = models.overrides[ovKey];
                  if (ov) ov.temperature = (e.target as HTMLInputElement).value;
                }}
                placeholder="Global default"
              />
            </div>
            <div class="settings-field override-field">
              <span class="override-label">Thinking</span>
              <div class="segmented-control">
                {#each thinkingOptions as opt (opt.value)}
                  <button
                    type="button"
                    class="seg-btn"
                    class:active={(models.overrides[ovKey]?.thinking ?? "") === opt.value}
                    onclick={() => {
                      ensureOverrides(ovKey);
                      const ov = models.overrides[ovKey];
                      if (ov) ov.thinking = opt.value;
                      models.overrides = models.overrides;
                    }}
                  >
                    {opt.label}
                  </button>
                {/each}
              </div>
            </div>
          </div>
        </div>
      {/each}
    </div>

    <div class="roles-section">
      <div class="roles-section-label">Embedding</div>
      <div class="role-row">
        <div class="role-row-label">
          Embedding
          <span class="role-tooltip">
            <span class="role-tooltip-icon">?</span>
            <span class="role-tooltip-text">{roleTooltips.embedding}</span>
          </span>
        </div>
        <div class="role-row-fields">
          <div class="settings-field">
            <label for="srole-embedding-provider">Provider</label>
            <select
              id="srole-embedding-provider"
              value={getProvider("embedding")}
              onchange={(e) => setProvider("embedding", (e.target as HTMLSelectElement).value)}
            >
              {#each providerNameOptions().filter((n) => {
                const entry = providerEntry(n);
                return entry && EMBEDDING_PROVIDERS.includes(entry.type);
              }) as pk (pk)}
                <option value={pk}>{pk}</option>
              {/each}
            </select>
          </div>
          <div class="settings-field">
            <label for="srole-embedding-model">Model</label>
            <div class="model-select-wrap">
              <select
                id="srole-embedding-model"
                value={otherActive["embedding"] ? "__other__" : getModel("embedding")}
                onchange={(e) => setModel("embedding", (e.target as HTMLSelectElement).value)}
                disabled={modelLoading["embedding"]}
              >
                {#if modelLoading["embedding"]}
                  <option value="">Loading...</option>
                {:else}
                  {#each modelLists["embedding"] ?? [] as m (m.id)}
                    <option value={m.id}>{m.name ?? m.id}</option>
                  {/each}
                  <option value="__other__">Other...</option>
                {/if}
              </select>
              {#if otherActive["embedding"]}
                <input
                  class="model-other-input"
                  type="text"
                  placeholder="Enter model ID..."
                  value={otherValues["embedding"] ?? ""}
                  oninput={(e) => setOtherModel("embedding", (e.target as HTMLInputElement).value)}
                />
              {/if}
            </div>
          </div>
        </div>
      </div>
    </div>

    <div class="roles-section">
      <div class="roles-section-label">Background Tasks</div>
      {#each bgTiers as role (role)}
        {@const ovKey = modelsKey(role)}
        <div class="role-row">
          <div class="role-row-label">
            {roleLabel(role)}
            <span class="role-tooltip">
              <span class="role-tooltip-icon">?</span>
              <span class="role-tooltip-text">{roleTooltips[role]}</span>
            </span>
          </div>
          <div class="role-row-fields">
            <div class="settings-field">
              <label for="srole-{role}-provider">Provider</label>
              <select
                id="srole-{role}-provider"
                value={getProvider(role)}
                onchange={(e) => setProvider(role, (e.target as HTMLSelectElement).value)}
              >
                {#each providerNameOptions() as pk (pk)}
                  <option value={pk}>{pk}</option>
                {/each}
              </select>
            </div>
            <div class="settings-field">
              <label for="srole-{role}-model">Model</label>
              <div class="model-select-wrap">
                <select
                  id="srole-{role}-model"
                  value={otherActive[role] ? "__other__" : getModel(role)}
                  onchange={(e) => setModel(role, (e.target as HTMLSelectElement).value)}
                  disabled={modelLoading[role]}
                >
                  {#if modelLoading[role]}
                    <option value="">Loading...</option>
                  {:else}
                    {#each modelLists[role] ?? [] as m (m.id)}
                      <option value={m.id}>{m.name ?? m.id}</option>
                    {/each}
                    <option value="__other__">Other...</option>
                  {/if}
                </select>
                {#if otherActive[role]}
                  <input
                    class="model-other-input"
                    type="text"
                    placeholder="Enter model ID..."
                    value={otherValues[role] ?? ""}
                    oninput={(e) => setOtherModel(role, (e.target as HTMLInputElement).value)}
                  />
                {/if}
              </div>
            </div>
          </div>
          <div class="role-overrides">
            <div class="settings-field override-field">
              <label for="srole-{role}-temp">Temperature</label>
              <input
                id="srole-{role}-temp"
                type="number"
                step="0.1"
                min="0"
                max="2"
                value={models.overrides[ovKey]?.temperature ?? ""}
                oninput={(e) => {
                  ensureOverrides(ovKey);
                  const ov = models.overrides[ovKey];
                  if (ov) ov.temperature = (e.target as HTMLInputElement).value;
                }}
                placeholder="Global default"
              />
            </div>
            <div class="settings-field override-field">
              <span class="override-label">Thinking</span>
              <div class="segmented-control">
                {#each thinkingOptions as opt (opt.value)}
                  <button
                    type="button"
                    class="seg-btn"
                    class:active={(models.overrides[ovKey]?.thinking ?? "") === opt.value}
                    onclick={() => {
                      ensureOverrides(ovKey);
                      const ov = models.overrides[ovKey];
                      if (ov) ov.thinking = opt.value;
                      models.overrides = models.overrides;
                    }}
                  >
                    {opt.label}
                  </button>
                {/each}
              </div>
            </div>
          </div>
        </div>
      {/each}
    </div>
  </div>
</div>
