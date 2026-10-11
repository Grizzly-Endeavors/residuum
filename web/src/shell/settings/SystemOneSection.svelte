<script lang="ts">
  import {
    fetchSystemOneModels,
    testSystemOne,
    type SystemOneForm,
    type SystemOneModelsResponse,
    type SystemOneTestResponse,
  } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { hub } from "../../lib/hub.svelte";
  import { Badge, Banner, Button, SelectField, TextField } from "../../lib/ui";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { configFieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The decision model (System 1) every agent shares: which service hosts
  // it, the model, and how to reach it. Auto Mode asks it about each tool
  // call. Test connection tries the values in the form, saved or not.

  let { scope, section }: AllSectionProps = $props();

  const uid = $props.id();
  const saved = $derived(scope.configFile.baseline);
  const provider = $derived(scope.config.system_one_provider);
  const status = $derived(hub.systemOne);

  const PROVIDERS = [
    { value: "", label: "None" },
    { value: "typesafe", label: "TypeSafe (Jev, hosted)" },
    { value: "ollama", label: "Ollama (decision models on your machine)" },
    { value: "other", label: "Other" },
  ] as const;

  const MODEL_HINTS: Readonly<Record<string, string>> = {
    typesafe: "Leave blank for jev-latest, the newest stable Jev.",
    ollama: "Pull one first, for example with: ollama pull nimble",
    other: "The model ID the service expects.",
  };

  /** What the form says, for the model list and the connection test. */
  function currentForm(): SystemOneForm {
    return {
      provider,
      url: scope.config.system_one_url,
      model: scope.config.system_one_model,
      api_key: scope.config.system_one_api_key,
      keep_alive: scope.config.system_one_keep_alive,
    };
  }

  function chooseProvider(next: string): void {
    if (next === scope.config.system_one_provider) return;
    scope.config.system_one_provider = next;
    // A model or address chosen for one service rarely exists on another.
    scope.config.system_one_model = "";
    if (next !== "other") scope.config.system_one_url = "";
    models = null;
    testResult = null;
  }

  let models = $state<SystemOneModelsResponse | null>(null);
  let listing = $state(false);
  async function listModels(): Promise<void> {
    listing = true;
    try {
      models = await fetchSystemOneModels(currentForm());
    } catch (err) {
      models = {
        models: [],
        error: userErrorMessage(err, { action: "Couldn't list the models." }),
      };
    } finally {
      listing = false;
    }
  }

  let testResult = $state<SystemOneTestResponse | null>(null);
  let testing = $state(false);
  async function runTest(): Promise<void> {
    testing = true;
    try {
      testResult = await testSystemOne(currentForm());
    } catch (err) {
      testResult = {
        ok: false,
        message: userErrorMessage(err, { action: "Couldn't run the test." }),
      };
    } finally {
      testing = false;
    }
  }
</script>

{#snippet mark()}
  {#if !status?.configured}
    <Badge dot>Not set up</Badge>
  {:else if status.outage !== null}
    <Badge tone="danger" dot>Not answering</Badge>
  {:else}
    <Badge tone="positive" dot>Set up</Badge>
  {/if}
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Decision model"
  lede="A fast model that answers yes-or-no and pick-one questions instead of writing text. Auto Mode uses it to check each tool call against an agent's rules. Every agent shares it."
>
  <SettingsGroup title="Service" status={mark}>
    {#if status?.outage}
      <Banner tone="warn">{status.outage.message}</Banner>
    {/if}
    <SelectField
      label="Provider"
      bind:value={() => provider, chooseProvider}
      options={PROVIDERS}
      hint="TypeSafe hosts Jev. Ollama 0.35 or later runs decision models such as Nimble and Tev1 on your machine. Other is any service with the same API."
      error={configFieldError(scope, "system_one_provider")}
    />

    {#if provider !== ""}
      {#if provider === "other"}
        <TextField
          label="Address"
          bind:value={scope.config.system_one_url}
          placeholder="https://decisions.example.com"
          autocomplete="off"
          spellcheck={false}
          code
          hint="The service's base address, without /v1/systemone."
          error={configFieldError(scope, "system_one_url")}
        />
      {/if}

      {#if provider !== "ollama"}
        <SecretConfigField
          label="API key"
          bind:value={scope.config.system_one_api_key}
          saved={saved.system_one_api_key}
          placeholder={provider === "typesafe" ? "Paste your TypeSafe API key" : "Optional"}
          error={configFieldError(scope, "system_one_api_key")}
        />
      {/if}

      <TextField
        label="Model"
        bind:value={scope.config.system_one_model}
        placeholder={provider === "typesafe" ? "jev-latest" : ""}
        list="{uid}-models"
        autocomplete="off"
        spellcheck={false}
        code
        hint={MODEL_HINTS[provider]}
        error={configFieldError(scope, "system_one_model")}
      />
      <datalist id="{uid}-models">
        {#each models?.models ?? [] as model (model.id)}
          <option value={model.id}>{model.description ?? ""}</option>
        {/each}
      </datalist>

      {#if provider === "ollama"}
        <TextField
          label="Keep the model loaded for"
          bind:value={scope.config.system_one_keep_alive}
          placeholder="5m"
          autocomplete="off"
          spellcheck={false}
          code
          hint="How long Ollama keeps the model in memory after a request, like 5m or 1h. Blank uses Ollama's default."
          error={configFieldError(scope, "system_one_keep_alive")}
        />
      {/if}

      <div class="system-one-actions">
        <Button size="sm" loading={listing} onclick={() => void listModels()}>
          {models === null ? "Show available models" : "Refresh models"}
        </Button>
        <Button size="sm" loading={testing} onclick={() => void runTest()}>Test connection</Button>
      </div>
      {#if models?.error}
        <Banner tone="error">{models.error}</Banner>
      {:else if models !== null}
        <p class="system-one-line" role="status">
          {models.models.length === 0
            ? "The service lists no models."
            : `Available: ${models.models.map((m) => m.id).join(", ")}.`}
        </p>
      {/if}
      {#if testResult}
        <Banner tone={testResult.ok ? "info" : "error"} icon={testResult.ok ? "check" : undefined}
          >{testResult.message}</Banner
        >
      {/if}
    {/if}
  </SettingsGroup>
</SettingsSection>

<style>
  .system-one-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .system-one-line {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }
</style>
