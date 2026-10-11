<script lang="ts">
  import { tick } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { PROVIDER_KEYS } from "../../components/setup/providers";
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import { envKeyOf, providerTypeLabel } from "../../lib/model-roles";
  import { envReferenceName, isSecretReference } from "../../lib/secrets";
  import type { FieldRef } from "../../lib/settings-fields";
  import type { AgentScopeModel } from "../../lib/settings-model.svelte";
  import type { SettingsProviderEntry } from "../../lib/types";
  import {
    Button,
    EmptyState,
    IconButton,
    SecretField,
    SelectField,
    TextField,
    type Choice,
  } from "../../lib/ui";
  import { fieldMark } from "./field-focus";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { fieldError } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";

  // The agent's providers: named connections to model services, each with
  // its type, key and address. A model names one before its slash. Adding,
  // editing and removing one are staged; Discard brings the list back.

  let { scope }: { scope: AgentScopeModel } = $props();

  const TYPES: readonly Choice[] = PROVIDER_KEYS.map((type) => ({
    value: type,
    label: providerTypeLabel(type),
  }));

  type ProviderField = "type" | "apiKey" | "url" | "keepAlive";

  /** Open providers, by name, so they stay open when a save gives the form new objects. */
  const open = new SvelteSet<string>();

  const blank = (): SettingsProviderEntry => ({
    name: "",
    type: "anthropic",
    apiKey: "",
    url: "",
    keepAlive: "",
  });

  let adding = $state(false);
  let draft = $state<SettingsProviderEntry>(blank());
  let tried = $state(false);
  let nameField = $state<HTMLInputElement | HTMLTextAreaElement>();
  let addButton = $state<HTMLButtonElement>();

  function nameProblem(name: string): string | null {
    const trimmed = name.trim();
    if (trimmed === "") return "Give it a name.";
    if (!/^[A-Za-z0-9_-]+$/.test(trimmed)) return "Use only letters, numbers, - and _.";
    if (scope.providers.some((entry) => entry.name === trimmed)) {
      return `There's already a provider called ${trimmed}.`;
    }
    return null;
  }
  const draftProblem = $derived(tried ? nameProblem(draft.name) : null);

  const hasProblems = (name: string): boolean =>
    scope.diagnostics.some(
      (placed) => placed.field?.kind === "provider" && placed.field.name === name,
    );

  function typeChoices(type: string): Choice[] {
    return TYPES.some((choice) => choice.value === type)
      ? [...TYPES]
      : [...TYPES, { value: type, label: type }];
  }

  /** The key on disk for a provider, which a key typed over it replaces. */
  function savedKey(name: string): string {
    return (
      scope.providersFile.baseline.providers.find((entry) => entry.name === name)?.apiKey ?? ""
    );
  }

  function summary(entry: SettingsProviderEntry): string {
    const parts = [providerTypeLabel(entry.type)];
    const variable = envReferenceName(entry.apiKey);
    if (isSecretReference(entry.apiKey)) parts.push("key stored securely");
    else if (variable !== null) parts.push(`key from ${variable}`);
    else if (entry.apiKey !== "") parts.push("new key, stored when you save");
    else if (entry.type !== "ollama") parts.push(`no key, so it reads ${envKeyOf(entry.type)}`);
    if (entry.url !== "") parts.push(entry.url);
    return parts.join(", ");
  }

  function remove(entry: SettingsProviderEntry): void {
    const providers = scope.providers;
    const at = providers.indexOf(entry);
    if (at < 0) return;
    const removed = $state.snapshot(entry);
    providers.splice(at, 1);
    notifyStagedRemoval(`Removed ${removed.name}. Save changes to keep it removed.`, () => {
      const now = scope.providers;
      if (!now.some((candidate) => candidate.name === removed.name)) {
        now.splice(Math.min(at, now.length), 0, removed);
      }
    });
  }

  async function startAdding(): Promise<void> {
    adding = true;
    await tick();
    nameField?.focus();
  }

  async function stopAdding(): Promise<void> {
    adding = false;
    draft = blank();
    tried = false;
    await tick();
    addButton?.focus();
  }

  function addDraft(): void {
    tried = true;
    if (nameProblem(draft.name) !== null) return;
    scope.providers.push({
      ...draft,
      name: draft.name.trim(),
      url: draft.url.trim(),
      keepAlive: "",
    });
    void stopAdding();
  }
</script>

{#snippet foot()}
  {#if !adding}
    <Button variant="quiet" size="sm" icon="plus" bind:element={addButton} onclick={startAdding}>
      Add a provider
    </Button>
  {/if}
{/snippet}

<SettingsGroup
  title="Providers"
  lede="Your accounts with the services that run models, and the key for each. A model names its provider before the slash. A service you haven't added here reads its key from the environment."
  {foot}
>
  {#if scope.providers.length === 0}
    <EmptyState>
      No providers added. The models above use each service's key from the environment.
    </EmptyState>
  {:else}
    <ul class="providers" aria-label="Providers">
      {#each scope.providers as entry (entry)}
        {@const expanded = open.has(entry.name) || hasProblems(entry.name)}
        {@const problem = fieldError(scope, { kind: "provider", name: entry.name })}
        <li class="provider">
          <div class="provider-head">
            <div class="provider-text">
              <code class="provider-name">{entry.name}</code>
              <span class="provider-line">{summary(entry)}</span>
            </div>
            <IconButton
              icon="edit"
              label="Edit {entry.name}"
              aria-expanded={expanded}
              onclick={() => {
                if (expanded) open.delete(entry.name);
                else open.add(entry.name);
              }}
            />
            <IconButton icon="trash" label="Remove {entry.name}" onclick={() => remove(entry)} />
          </div>
          {#if problem}<p class="provider-problem">{problem}</p>{/if}
          {#if expanded}
            {@const ref = (field: ProviderField): FieldRef => ({
              kind: "provider",
              name: entry.name,
              field,
            })}
            <div class="provider-fields">
              <div data-field={fieldMark(ref("type"))}>
                <SelectField
                  label="Type"
                  options={typeChoices(entry.type)}
                  error={fieldError(scope, ref("type"))}
                  bind:value={entry.type}
                />
              </div>
              <div data-field={fieldMark(ref("apiKey"))}>
                <SecretConfigField
                  label={entry.type === "ollama" ? "API key (optional)" : "API key"}
                  saved={savedKey(entry.name)}
                  hint="Stored securely when you save."
                  error={fieldError(scope, ref("apiKey"))}
                  bind:value={entry.apiKey}
                />
              </div>
              <div data-field={fieldMark(ref("url"))}>
                <TextField
                  label="Address"
                  code
                  autocomplete="off"
                  spellcheck="false"
                  hint="Leave it blank for {providerTypeLabel(entry.type)}'s usual address."
                  error={fieldError(scope, ref("url"))}
                  bind:value={entry.url}
                />
              </div>
              {#if entry.type === "ollama"}
                <div data-field={fieldMark(ref("keepAlive"))}>
                  <TextField
                    label="Keep the model loaded for"
                    code
                    placeholder="5m"
                    hint="How long Ollama keeps a model in memory after it answers, such as 5m or 1h."
                    error={fieldError(scope, ref("keepAlive"))}
                    bind:value={entry.keepAlive}
                  />
                </div>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if adding}
    <form
      class="provider-fields provider-add"
      aria-label="Add a provider"
      onsubmit={(event) => {
        event.preventDefault();
        addDraft();
      }}
    >
      <TextField
        label="Name"
        code
        autocomplete="off"
        spellcheck="false"
        hint="Models name it before the slash, as in work/gpt-4o."
        error={draftProblem ?? undefined}
        bind:value={draft.name}
        bind:element={nameField}
      />
      <SelectField label="Type" options={TYPES} bind:value={draft.type} />
      <SecretField
        label={draft.type === "ollama" ? "API key (optional)" : "API key"}
        source={{ kind: "none" }}
        hint="Stored securely when you save."
        bind:value={draft.apiKey}
      />
      <TextField
        label="Address"
        code
        autocomplete="off"
        spellcheck="false"
        hint="Leave it blank for {providerTypeLabel(draft.type)}'s usual address."
        bind:value={draft.url}
      />
      <div class="provider-add-actions">
        <Button variant="primary" type="submit">Add provider</Button>
        <Button variant="quiet" onclick={() => void stopAdding()}>Cancel</Button>
      </div>
    </form>
  {/if}
</SettingsGroup>

<style>
  .providers {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .provider {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-10) 0;

    & + .provider {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .provider-head {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .provider-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .provider-name {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .provider-line {
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }

  .provider-problem {
    color: var(--color-err-text);
    font-size: var(--font-size-xs);
  }

  .provider-fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .provider-add {
    padding-top: var(--space-12);
    border-top: 1px solid var(--color-line-soft);
  }

  .provider-add-actions {
    display: flex;
    gap: var(--space-8);
  }
</style>
