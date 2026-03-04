<script lang="ts">
  import { onMount } from "svelte";
  import type { SetupWizardState, ProviderKey } from "../../lib/types";
  import {
    fetchModels,
    DEFAULT_MODELS,
    DEFAULT_EMBEDDING_MODELS,
    EMBEDDING_PROVIDERS,
    debounce,
    type ModelEntry,
  } from "../../lib/models";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState, onNext, onBack }: Props = $props();

  const providers: Record<string, string> = {
    anthropic: "Anthropic",
    openai: "OpenAI",
    gemini: "Google Gemini",
    ollama: "Ollama",
  };

  const roleTooltips: Record<string, string> = {
    main: "The primary model used for conversations and task execution.",
    observer: "Watches conversations and extracts facts, preferences, and patterns for long-term memory.",
    reflector: "Periodically reviews stored memories, consolidates duplicates, and resolves contradictions.",
    pulse: "Drives proactive behavior — daily briefings, check-ins, and ambient monitoring tasks.",
    embedding: "Generates vector embeddings for semantic memory search. Only some providers support this.",
    "bg-small": "Used for lightweight background tasks like formatting, simple lookups, and notifications.",
    "bg-medium": "Used for moderate background tasks like summarization and analysis.",
    "bg-large": "Used for complex background tasks that need strong reasoning ability.",
  };

  // Track model lists per role
  let modelLists = $state<Record<string, ModelEntry[]>>({});
  let modelLoading = $state<Record<string, boolean>>({});
  let otherActive = $state<Record<string, boolean>>({});
  let otherValues = $state<Record<string, string>>({});

  function getRoleProvider(role: string): string {
    if (role === "main") return wizardState.mainProvider;
    if (role === "embedding") {
      return wizardState.embeddingModel.provider || defaultEmbeddingProvider();
    }
    if (role.startsWith("bg-")) {
      const tier = role.slice(3);
      return wizardState.backgroundModels[tier].provider || wizardState.mainProvider;
    }
    return wizardState.roles[role]?.provider || wizardState.mainProvider;
  }

  function getRoleModel(role: string): string {
    if (role === "main") return wizardState.providerConfigs[wizardState.mainProvider].model;
    if (role === "embedding") return wizardState.embeddingModel.model;
    if (role.startsWith("bg-")) return wizardState.backgroundModels[role.slice(3)].model;
    return wizardState.roles[role]?.model || "";
  }

  function setRoleProvider(role: string, prov: string) {
    if (role === "main") {
      wizardState.mainProvider = prov as ProviderKey;
      wizardState.providerConfigs[prov as ProviderKey].model = "";
    } else if (role === "embedding") {
      wizardState.embeddingModel.provider = prov;
      wizardState.embeddingModel.model = "";
    } else if (role.startsWith("bg-")) {
      const tier = role.slice(3);
      wizardState.backgroundModels[tier].provider = prov;
      wizardState.backgroundModels[tier].model = "";
    } else {
      wizardState.roles[role].provider = prov;
      wizardState.roles[role].model = "";
    }
    otherActive[role] = false;
    otherValues[role] = "";
    loadModels(role);
  }

  function setRoleModel(role: string, value: string) {
    if (value === "__other__") {
      otherActive[role] = true;
      return;
    }
    otherActive[role] = false;
    if (role === "main") {
      wizardState.providerConfigs[wizardState.mainProvider].model = value;
    } else if (role === "embedding") {
      wizardState.embeddingModel.model = value;
    } else if (role.startsWith("bg-")) {
      wizardState.backgroundModels[role.slice(3)].model = value;
    } else {
      wizardState.roles[role].model = value;
    }
  }

  function setOtherModel(role: string, value: string) {
    otherValues[role] = value;
    if (role === "main") {
      wizardState.providerConfigs[wizardState.mainProvider].model = value;
    } else if (role === "embedding") {
      wizardState.embeddingModel.model = value;
    } else if (role.startsWith("bg-")) {
      wizardState.backgroundModels[role.slice(3)].model = value;
    } else {
      wizardState.roles[role].model = value;
    }
  }

  function defaultEmbeddingProvider(): string {
    return (
      wizardState.selectedProviders.find((p) => EMBEDDING_PROVIDERS.includes(p)) ||
      EMBEDDING_PROVIDERS[0] ||
      ""
    );
  }

  let hasEmbeddingProvider = $derived(
    wizardState.selectedProviders.some((p) => EMBEDDING_PROVIDERS.includes(p))
  );

  let mainProviderOptions = $derived([...wizardState.selectedProviders]);

  let embeddingProviderOptions = $derived(
    EMBEDDING_PROVIDERS.filter((p) =>
      wizardState.selectedProviders.includes(p as ProviderKey)
    )
  );

  async function loadModels(role: string) {
    const prov = getRoleProvider(role);
    const provCfg = wizardState.providerConfigs[prov as ProviderKey];
    const apiKey = prov !== "ollama" ? provCfg?.apiKey : undefined;
    const url = provCfg?.url || undefined;

    modelLoading[role] = true;
    const result = await fetchModels(prov, apiKey, url);
    modelLists[role] = result.models;
    modelLoading[role] = false;

    // Auto-select default if no model set
    const current = getRoleModel(role);
    if (!current && result.models.length > 0) {
      const defaultModel =
        role === "embedding"
          ? DEFAULT_EMBEDDING_MODELS[prov] || ""
          : DEFAULT_MODELS[prov] || "";
      const found = result.models.some((m) => m.id === defaultModel);
      const autoModel = found ? defaultModel : result.models[0].id;
      setRoleModel(role, autoModel);
    }
  }

  const debouncedLoadModels = debounce((role: string) => {
    loadModels(role);
  }, 500);

  const allRoles = ["main", "observer", "reflector", "pulse"];
  const bgTiers = ["small", "medium", "large"];

  onMount(() => {
    for (const role of allRoles) loadModels(role);
    if (hasEmbeddingProvider) loadModels("embedding");
    for (const tier of bgTiers) loadModels(`bg-${tier}`);
  });

  function roleRow(role: string): { label: string; providerOptions: string[] } {
    if (role === "main") {
      return { label: "Main Agent", providerOptions: [...wizardState.selectedProviders] };
    }
    if (role === "embedding") {
      return {
        label: "Embedding",
        providerOptions: EMBEDDING_PROVIDERS.filter((p) =>
          wizardState.selectedProviders.includes(p as ProviderKey)
        ),
      };
    }
    const labels: Record<string, string> = {
      observer: "Observer",
      reflector: "Reflector",
      pulse: "Pulse",
      "bg-small": "Small",
      "bg-medium": "Medium",
      "bg-large": "Large",
    };
    return {
      label: labels[role] || role,
      providerOptions: Object.keys(providers),
    };
  }
</script>

<h2>Assign Models</h2>
<p class="subtitle">Choose which model to use for each role. All default to the main model if left unchanged.</p>

<!-- Agent section -->
<div class="roles-section">
  <div class="roles-section-label">Agent</div>
  <div class="role-row">
    <div class="role-row-label">
      Main Agent
      <span class="role-tooltip">
        <span class="role-tooltip-icon">?</span>
        <span class="role-tooltip-text">{roleTooltips.main}</span>
      </span>
    </div>
    <div class="role-row-fields">
      <div class="settings-field">
        <label>Provider</label>
        <select
          value={getRoleProvider("main")}
          onchange={(e) => setRoleProvider("main", (e.target as HTMLSelectElement).value)}
        >
          {#each mainProviderOptions as pk}
            <option value={pk}>{providers[pk]}</option>
          {/each}
        </select>
      </div>
      <div class="settings-field">
        <label>Model</label>
        <div class="model-select-wrap">
          <select
            value={otherActive["main"] ? "__other__" : getRoleModel("main")}
            onchange={(e) => setRoleModel("main", (e.target as HTMLSelectElement).value)}
            disabled={modelLoading["main"]}
          >
            {#if modelLoading["main"]}
              <option value="">Loading...</option>
            {:else}
              {#each modelLists["main"] || [] as m}
                <option value={m.id}>{m.name || m.id}</option>
              {/each}
              <option value="__other__">Other...</option>
            {/if}
          </select>
          {#if otherActive["main"]}
            <input
              class="model-other-input"
              type="text"
              placeholder="Enter model ID..."
              value={otherValues["main"] || ""}
              oninput={(e) => setOtherModel("main", (e.target as HTMLInputElement).value)}
            />
          {/if}
        </div>
      </div>
    </div>
  </div>
</div>

<!-- Subsystems section -->
<div class="roles-section">
  <div class="roles-section-label">Subsystems</div>
  <p class="roles-section-hint">Memory and proactivity subsystems. These can use smaller, cheaper models.</p>
  {#each ["observer", "reflector", "pulse"] as role}
    {@const info = roleRow(role)}
    <div class="role-row">
      <div class="role-row-label">
        {info.label}
        <span class="role-tooltip">
          <span class="role-tooltip-icon">?</span>
          <span class="role-tooltip-text">{roleTooltips[role]}</span>
        </span>
      </div>
      <div class="role-row-fields">
        <div class="settings-field">
          <label>Provider</label>
          <select
            value={getRoleProvider(role)}
            onchange={(e) => setRoleProvider(role, (e.target as HTMLSelectElement).value)}
          >
            {#each info.providerOptions as pk}
              <option value={pk}>{providers[pk]}</option>
            {/each}
          </select>
        </div>
        <div class="settings-field">
          <label>Model</label>
          <div class="model-select-wrap">
            <select
              value={otherActive[role] ? "__other__" : getRoleModel(role)}
              onchange={(e) => setRoleModel(role, (e.target as HTMLSelectElement).value)}
              disabled={modelLoading[role]}
            >
              {#if modelLoading[role]}
                <option value="">Loading...</option>
              {:else}
                {#each modelLists[role] || [] as m}
                  <option value={m.id}>{m.name || m.id}</option>
                {/each}
                <option value="__other__">Other...</option>
              {/if}
            </select>
            {#if otherActive[role]}
              <input
                class="model-other-input"
                type="text"
                placeholder="Enter model ID..."
                value={otherValues[role] || ""}
                oninput={(e) => setOtherModel(role, (e.target as HTMLInputElement).value)}
              />
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/each}
</div>

<!-- Embedding section -->
{#if hasEmbeddingProvider}
  <div class="roles-section">
    <div class="roles-section-label">Embedding</div>
    <p class="roles-section-hint">Used for semantic memory search. Anthropic does not offer embeddings.</p>
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
          <label>Provider</label>
          <select
            value={getRoleProvider("embedding")}
            onchange={(e) => setRoleProvider("embedding", (e.target as HTMLSelectElement).value)}
          >
            {#each embeddingProviderOptions as pk}
              <option value={pk}>{providers[pk]}</option>
            {/each}
          </select>
        </div>
        <div class="settings-field">
          <label>Model</label>
          <div class="model-select-wrap">
            <select
              value={otherActive["embedding"] ? "__other__" : getRoleModel("embedding")}
              onchange={(e) => setRoleModel("embedding", (e.target as HTMLSelectElement).value)}
              disabled={modelLoading["embedding"]}
            >
              {#if modelLoading["embedding"]}
                <option value="">Loading...</option>
              {:else}
                {#each modelLists["embedding"] || [] as m}
                  <option value={m.id}>{m.name || m.id}</option>
                {/each}
                <option value="__other__">Other...</option>
              {/if}
            </select>
            {#if otherActive["embedding"]}
              <input
                class="model-other-input"
                type="text"
                placeholder="Enter model ID..."
                value={otherValues["embedding"] || ""}
                oninput={(e) => setOtherModel("embedding", (e.target as HTMLInputElement).value)}
              />
            {/if}
          </div>
        </div>
      </div>
    </div>
  </div>
{/if}

<!-- Background section -->
<div class="roles-section">
  <div class="roles-section-label">Background Tasks</div>
  <p class="roles-section-hint">Tiered models for background work. Tasks specify small, medium, or large.</p>
  {#each bgTiers as tier}
    {@const role = `bg-${tier}`}
    {@const info = roleRow(role)}
    <div class="role-row">
      <div class="role-row-label">
        {info.label}
        <span class="role-tooltip">
          <span class="role-tooltip-icon">?</span>
          <span class="role-tooltip-text">{roleTooltips[role]}</span>
        </span>
      </div>
      <div class="role-row-fields">
        <div class="settings-field">
          <label>Provider</label>
          <select
            value={getRoleProvider(role)}
            onchange={(e) => setRoleProvider(role, (e.target as HTMLSelectElement).value)}
          >
            {#each info.providerOptions as pk}
              <option value={pk}>{providers[pk]}</option>
            {/each}
          </select>
        </div>
        <div class="settings-field">
          <label>Model</label>
          <div class="model-select-wrap">
            <select
              value={otherActive[role] ? "__other__" : getRoleModel(role)}
              onchange={(e) => setRoleModel(role, (e.target as HTMLSelectElement).value)}
              disabled={modelLoading[role]}
            >
              {#if modelLoading[role]}
                <option value="">Loading...</option>
              {:else}
                {#each modelLists[role] || [] as m}
                  <option value={m.id}>{m.name || m.id}</option>
                {/each}
                <option value="__other__">Other...</option>
              {/if}
            </select>
            {#if otherActive[role]}
              <input
                class="model-other-input"
                type="text"
                placeholder="Enter model ID..."
                value={otherValues[role] || ""}
                oninput={(e) => setOtherModel(role, (e.target as HTMLInputElement).value)}
              />
            {/if}
          </div>
        </div>
      </div>
    </div>
  {/each}
</div>

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack}>Back</button>
  <button class="btn btn-primary" onclick={onNext}>Next</button>
</div>
