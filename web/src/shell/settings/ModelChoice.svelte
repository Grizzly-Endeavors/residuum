<script lang="ts">
  import { tick, untrack } from "svelte";
  import {
    directKeyNote,
    providerOptions,
    providerTypeLabel,
    splitModel,
    startingModel,
    unsetChoiceLabel,
  } from "../../lib/model-roles";
  import { EMBEDDING_MODEL_LISTS, fetchModels, type ModelEntry } from "../../lib/models";
  import { router } from "../../lib/router.svelte";
  import type { AgentScopeModel } from "../../lib/settings-model.svelte";
  import type { ModelRoleKey } from "../../lib/types";
  import { Button, SelectField, TextField, type Choice } from "../../lib/ui";
  import { fieldMark } from "./field-focus";
  import { fieldError } from "./sections";

  // A role's provider and model, from its `provider/model` value. The model
  // list is the provider's own, loaded when the role shows, or a list of
  // common models with a note when that can't be read; "Other model" takes
  // any id. A role with a failover list keeps the models after the first,
  // and says which they are.

  interface Props {
    scope: AgentScopeModel;
    role: ModelRoleKey;
  }

  let { scope, role }: Props = $props();

  /** The model select's value for a model the list doesn't have. */
  const OTHER = "__other__";
  /** How long the list waits after its provider's key or address last changed, so typing one doesn't ask on every key. */
  const RELIST_DELAY_MS = 400;

  const value = $derived(scope.models[role]);
  const parts = $derived(splitModel(value));
  const options = $derived(providerOptions(scope.providers, role));
  const chosen = $derived(
    value === "" ? null : (options.find((option) => option.name === parts.provider) ?? null),
  );
  const unknownProvider = $derived(value !== "" && chosen === null);
  const providerLabel = $derived(
    chosen === null ? parts.provider : (chosen.entry?.name ?? providerTypeLabel(chosen.type)),
  );

  const providerChoices = $derived<Choice[]>([
    ...(role === "main" ? [] : [{ value: "", label: unsetChoiceLabel(role, scope.models) }]),
    ...options.map((option) => ({ value: option.name, label: option.label })),
    ...(unknownProvider
      ? [{ value: parts.provider, label: `${parts.provider} (not set up)` }]
      : []),
  ]);

  // What the list is read with: changes to anything else about the provider don't read it again.
  const listSource = $derived(
    chosen === null
      ? null
      : { type: chosen.type, key: chosen.entry?.apiKey ?? "", url: chosen.entry?.url ?? "" },
  );
  const listKey = $derived(listSource === null ? null : JSON.stringify(listSource));

  let list = $state<ModelEntry[]>([]);
  let listError = $state<string | null>(null);
  let listLoading = $state(false);
  let listedOnce = false;

  $effect(() => {
    const source = listKey === null ? null : untrack(() => listSource);
    if (source === null) {
      list = [];
      listError = null;
      listLoading = false;
      return;
    }
    const { type, key, url } = source;
    if (role === "embedding") {
      // Providers don't list their embedding models, so the common ones are the list.
      list = EMBEDDING_MODEL_LISTS[type] ?? [];
      listError = null;
      listLoading = false;
      return;
    }
    let current = true;
    listLoading = true;
    const timer = setTimeout(
      () => {
        void fetchModels(scope.agent, type, key || undefined, url || undefined).then((result) => {
          if (!current) return;
          list = result.models;
          listError = result.error;
          listLoading = false;
        });
      },
      listedOnce ? RELIST_DELAY_MS : 0,
    );
    listedOnce = true;
    return () => {
      current = false;
      clearTimeout(timer);
    };
  });

  let typingOther = $state(false);
  let otherField = $state<HTMLInputElement | HTMLTextAreaElement>();
  const listed = $derived(list.some((entry) => entry.id === parts.model));
  const unlisted = $derived(parts.model !== "" && !listed);

  const modelChoices = $derived<Choice[]>([
    ...(unlisted && !typingOther
      ? [
          {
            value: parts.model,
            label:
              listLoading || listError !== null ? parts.model : `${parts.model} (not in the list)`,
          },
        ]
      : []),
    ...list.map((entry) => ({ value: entry.id, label: entry.name || entry.id })),
    { value: OTHER, label: "Other model…" },
  ]);

  const modelHint = $derived.by((): string | undefined => {
    if (listLoading) return `Loading ${providerLabel}'s models…`;
    if (listError !== null) {
      return `Couldn't load ${providerLabel}'s models (${listError}), so these are common ones. Choose Other model for any other.`;
    }
    return undefined;
  });

  const fallbacks = $derived(value === "" ? [] : (scope.models.fallbacks[role] ?? []));

  function chooseProvider(name: string): void {
    typingOther = false;
    if (name === "") {
      scope.models[role] = "";
      return;
    }
    const option = options.find((candidate) => candidate.name === name);
    scope.models[role] = `${name}/${option === undefined ? "" : startingModel(role, option.type)}`;
  }

  async function chooseModel(model: string): Promise<void> {
    if (model === OTHER) {
      typingOther = true;
      await tick();
      otherField?.focus();
      return;
    }
    typingOther = false;
    scope.models[role] = `${parts.provider}/${model}`;
  }
</script>

<div class="model-choice" data-field={fieldMark({ kind: "role", role })}>
  <div class="model-choice-pair">
    <SelectField
      label="Provider"
      options={providerChoices}
      placeholder={role === "main" && value === ""
        ? unsetChoiceLabel(role, scope.models)
        : undefined}
      hint={role === "main" && chosen !== null ? directKeyNote(chosen) : undefined}
      error={unknownProvider
        ? `There's no provider called ${parts.provider}. Add it under Providers, or choose another.`
        : undefined}
      bind:value={() => (value === "" ? "" : parts.provider), chooseProvider}
    />
    {#if value !== ""}
      <SelectField
        label="Model"
        options={modelChoices}
        hint={modelHint}
        error={fieldError(scope, { kind: "role", role })}
        bind:value={() => (typingOther ? OTHER : parts.model), (next) => void chooseModel(next)}
      />
    {/if}
  </div>
  {#if typingOther && value !== ""}
    <TextField
      label="Model id"
      code
      autocomplete="off"
      spellcheck="false"
      hint="As {providerLabel} names it, such as the id in its model list."
      bind:element={otherField}
      bind:value={
        () => parts.model,
        (next) => {
          scope.models[role] = `${parts.provider}/${next}`;
        }
      }
    />
  {/if}
  {#if fallbacks.length > 0}
    <div class="model-choice-failover">
      <p>
        If this model fails, {scope.agent} tries
        {#each fallbacks as fallback, index (index)}{#if index > 0}, then
          {/if}<code>{fallback}</code>{/each}.
      </p>
      <Button variant="quiet" size="sm" onclick={() => void router.switchSettingsSection("raw")}>
        Edit the list in Raw config
      </Button>
    </div>
  {/if}
</div>

<style>
  .model-choice {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    min-width: 0;
    container-type: inline-size;
  }

  .model-choice-pair {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-12) var(--space-16);
    align-items: start;
  }

  @container (max-width: 460px) {
    .model-choice-pair {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  .model-choice-failover {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-12);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);

    & p {
      flex: 1 1 260px;
      overflow-wrap: anywhere;
    }

    & code {
      color: var(--color-text);
    }
  }
</style>
