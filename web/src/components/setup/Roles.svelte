<script lang="ts">
  import { onMount } from "svelte";
  import type { SetupWizardState, ProviderKey } from "../../lib/types";
  import {
    fetchModels,
    DEFAULT_MODELS,
    DEFAULT_EMBEDDING_MODELS,
    EMBEDDING_PROVIDERS,
    EMBEDDING_MODEL_LISTS,
    type ModelEntry,
  } from "../../lib/models";
  import { Banner, SelectField, TextField, type Choice } from "../../lib/ui";
  import { providerName } from "./providers";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState = $bindable(), onNext, onBack }: Props = $props();

  const uid = $props.id();

  /** The model select's value for "a model that isn't listed". */
  const OTHER = "__other__";

  const ROLES: Record<string, { name: string; description: string }> = {
    main: { name: "Main agent", description: "Holds your conversations and does the work." },
    observer: {
      name: "Observer",
      description: "Watches conversations and saves facts, preferences and patterns to memory.",
    },
    reflector: {
      name: "Reflector",
      description:
        "Reviews saved memories now and then, merging duplicates and settling conflicts.",
    },
    pulse: {
      name: "Pulse",
      description: "Drives proactive work: daily briefings, check-ins and background monitoring.",
    },
    embedding: {
      name: "Embedding",
      description: "Turns text into vectors for memory search. Anthropic doesn't offer embeddings.",
    },
    "bg-small": {
      name: "Small",
      description: "Light jobs, like formatting, simple lookups and notifications.",
    },
    "bg-medium": { name: "Medium", description: "Moderate jobs, like summaries and analysis." },
    "bg-large": { name: "Large", description: "Hard jobs that need strong reasoning." },
  };

  // Track model lists per role
  let modelLists = $state<Record<string, ModelEntry[]>>({});
  let modelLoading = $state<Record<string, boolean>>({});
  let modelErrors = $state<Record<string, string | null>>({});
  let otherActive = $state<Record<string, boolean>>({});
  let otherValues = $state<Record<string, string>>({});

  // A role's stored provider can go stale (or start unset) when the
  // Providers step is revisited and a provider gets deselected — fall back
  // to the main provider rather than surfacing a picker pointed at a
  // provider with no `providers.toml` section.
  function validProvider(candidate: string | undefined): string | undefined {
    return candidate && wizardState.selectedProviders.includes(candidate as ProviderKey)
      ? candidate
      : undefined;
  }

  function getRoleProvider(role: string): string {
    if (role === "main") return wizardState.mainProvider;
    if (role === "embedding") {
      return validProvider(wizardState.embeddingModel.provider) ?? defaultEmbeddingProvider();
    }
    if (role.startsWith("bg-")) {
      const tier = role.slice(3);
      return (
        validProvider(wizardState.backgroundModels[tier]?.provider) ?? wizardState.mainProvider
      );
    }
    return validProvider(wizardState.roles[role]?.provider) ?? wizardState.mainProvider;
  }

  function getRoleModel(role: string): string {
    if (role === "main") return wizardState.providerConfigs[wizardState.mainProvider].model;
    if (role === "embedding") return wizardState.embeddingModel.model;
    if (role.startsWith("bg-")) return wizardState.backgroundModels[role.slice(3)]?.model ?? "";
    return wizardState.roles[role]?.model ?? "";
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
      const bg = wizardState.backgroundModels[tier];
      if (bg) {
        bg.provider = prov;
        bg.model = "";
      }
    } else {
      const r = wizardState.roles[role];
      if (r) {
        r.provider = prov;
        r.model = "";
      }
    }
    otherActive[role] = false;
    otherValues[role] = "";
    void loadModels(role);
  }

  function writeRoleModel(role: string, value: string) {
    if (role === "main") {
      wizardState.providerConfigs[wizardState.mainProvider].model = value;
    } else if (role === "embedding") {
      wizardState.embeddingModel.model = value;
    } else if (role.startsWith("bg-")) {
      const bg = wizardState.backgroundModels[role.slice(3)];
      if (bg) bg.model = value;
    } else {
      const r = wizardState.roles[role];
      if (r) r.model = value;
    }
  }

  function setRoleModel(role: string, value: string) {
    if (value === OTHER) {
      otherActive[role] = true;
      return;
    }
    otherActive[role] = false;
    writeRoleModel(role, value);
  }

  function setOtherModel(role: string, value: string) {
    otherValues[role] = value;
    writeRoleModel(role, value);
  }

  function defaultEmbeddingProvider(): string {
    return (
      wizardState.selectedProviders.find((p) => EMBEDDING_PROVIDERS.includes(p)) ??
      EMBEDDING_PROVIDERS[0] ??
      ""
    );
  }

  let hasEmbeddingProvider = $derived(
    wizardState.selectedProviders.some((p) => EMBEDDING_PROVIDERS.includes(p)),
  );

  /** With the list loaded, pick the default for an unset role, or show a model typed under Other again. */
  function settleModel(role: string, models: ModelEntry[], defaultModel: string) {
    const current = getRoleModel(role);
    if (!current) {
      if (models.length === 0) return;
      const found = models.some((m) => m.id === defaultModel);
      setRoleModel(role, found ? defaultModel : (models[0]?.id ?? ""));
    } else if (!models.some((m) => m.id === current)) {
      otherActive[role] = true;
      otherValues[role] = current;
    }
  }

  async function loadModels(role: string) {
    const prov = getRoleProvider(role);

    // Embedding models are never returned by provider APIs — use hardcoded lists
    if (role === "embedding") {
      const models = EMBEDDING_MODEL_LISTS[prov] ?? [];
      // Persist the implied default so the generated providers.toml includes it.
      if (wizardState.embeddingModel.provider === "" && prov !== "") {
        wizardState.embeddingModel.provider = prov;
      }
      modelLists[role] = models;
      modelErrors[role] = null;
      settleModel(role, models, DEFAULT_EMBEDDING_MODELS[prov] ?? "");
      return;
    }

    const provCfg = wizardState.providerConfigs[prov as ProviderKey];
    const apiKey = prov !== "ollama" ? provCfg?.apiKey : undefined;
    const url = provCfg?.url ?? undefined;

    modelLoading[role] = true;
    // No agent exists yet, so the hub looks the models up.
    const result = await fetchModels(null, prov, apiKey, url);
    modelLists[role] = result.models;
    modelLoading[role] = false;
    modelErrors[role] = result.error;
    settleModel(role, result.models, DEFAULT_MODELS[prov] ?? "");
  }

  const subsystemRoles = ["observer", "reflector", "pulse"];
  const backgroundRoles = ["bg-small", "bg-medium", "bg-large"];

  onMount(() => {
    for (const role of ["main", ...subsystemRoles]) void loadModels(role);
    if (hasEmbeddingProvider) void loadModels("embedding");
    for (const role of backgroundRoles) void loadModels(role);
  });

  function providerChoices(role: string): Choice[] {
    const keys =
      role === "embedding"
        ? EMBEDDING_PROVIDERS.filter((p) =>
            wizardState.selectedProviders.includes(p as ProviderKey),
          )
        : wizardState.selectedProviders;
    return keys.map((key) => ({ value: key, label: providerName(key) }));
  }

  function modelChoices(role: string): Choice[] {
    return [
      ...(modelLists[role] ?? []).map((m) => ({ value: m.id, label: m.name || m.id })),
      { value: OTHER, label: "Other…" },
    ];
  }
</script>

{#snippet roleRow(role: string)}
  <div
    class="setup-role"
    role="group"
    aria-labelledby="{uid}-{role}"
    aria-describedby="{uid}-{role}-desc"
  >
    <div class="setup-role-head">
      <span id="{uid}-{role}" class="setup-role-name">{ROLES[role]?.name}</span>
      <span id="{uid}-{role}-desc" class="setup-role-desc">{ROLES[role]?.description}</span>
    </div>
    <div class="setup-role-fields">
      <SelectField
        label="Provider"
        options={providerChoices(role)}
        bind:value={() => getRoleProvider(role), (value) => setRoleProvider(role, value)}
      />
      <SelectField
        label="Model"
        options={modelChoices(role)}
        loading={modelLoading[role] ?? false}
        bind:value={
          () => (otherActive[role] ? OTHER : getRoleModel(role)),
          (value) => setRoleModel(role, value)
        }
      />
    </div>
    {#if otherActive[role]}
      <TextField
        label="Model ID"
        placeholder="Enter the model's ID"
        code
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        bind:value={() => otherValues[role] ?? "", (value) => setOtherModel(role, value)}
      />
    {/if}
    {#if modelErrors[role]}
      <Banner tone="warn">
        Couldn't load {providerName(getRoleProvider(role))}'s models ({modelErrors[role]}), so this
        is a fallback list. Go back to Providers to check the API key or base URL.
      </Banner>
    {/if}
  </div>
{/snippet}

<SetupGroup title="Agent">
  {@render roleRow("main")}
</SetupGroup>

<SetupGroup title="Memory and proactive work" hint="These can use smaller, cheaper models.">
  {#each subsystemRoles as role (role)}
    {@render roleRow(role)}
  {/each}
</SetupGroup>

{#if hasEmbeddingProvider}
  <SetupGroup title="Memory search">
    {@render roleRow("embedding")}
  </SetupGroup>
{/if}

<SetupGroup
  title="Background tasks"
  hint="Each background task asks for a small, medium or large model."
>
  {#each backgroundRoles as role (role)}
    {@render roleRow(role)}
  {/each}
</SetupGroup>

<SetupNav {onBack} {onNext} />

<style>
  .setup-role {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);

    & + & {
      padding-top: var(--space-16);
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .setup-role-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .setup-role-name {
    font-weight: var(--font-weight-medium);
  }

  .setup-role-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .setup-role-fields {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: var(--space-12);
  }

  @container setup-group (max-width: 440px) {
    .setup-role-fields {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
