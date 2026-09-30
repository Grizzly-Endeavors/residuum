<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { fetchProvidersRaw } from "../lib/api";
  import { agentConfigFile, configCoordinator } from "../lib/config-coordinator";
  import { requireAgent } from "../lib/paths";
  import { parseProvidersToml, modelRoleJson } from "../lib/settings-toml";
  import { fetchModels, type ModelEntry } from "../lib/models";
  import { toast } from "../lib/toast.svelte";
  import { userErrorMessage } from "../lib/errors";
  import { clickOutside } from "../lib/actions/clickOutside";

  let { disabled = false }: { disabled?: boolean } = $props();

  let open = $state(false);
  let currentModel = $state("");
  let currentProvider = $state("");
  let models = $state<ModelEntry[]>([]);
  let saving = $state(false);
  let settled = $state(false);

  onMount(async () => {
    // Give config a short window to load before we show the "no model" fallback.
    // Prevents a brief error-red flash on first paint while providers.toml is fetched.
    const settleTimer = window.setTimeout(() => {
      settled = true;
    }, 300);
    await loadCurrentModel();
    window.clearTimeout(settleTimer);
    settled = true;
  });

  // The chip shows what providers.toml says, so it follows the file: a change
  // made in settings, by history restore, or outside this page reloads it.
  $effect(() => {
    const agent = ws.agent;
    if (agent === null) return;
    return configCoordinator.subscribe(agentConfigFile(agent, "providers"), () => {
      void loadCurrentModel();
    });
  });

  /** Sequences loads, so a slow one can't overwrite what a later one found. */
  let loadCount = 0;

  async function loadCurrentModel(): Promise<void> {
    const load = ++loadCount;
    try {
      const agent = requireAgent(ws.agent);
      const raw = await fetchProvidersRaw(agent);
      const parsed = parseProvidersToml(raw);
      const mainValue = parsed.models.main;
      if (load !== loadCount) return;
      if (!mainValue) {
        currentProvider = "";
        currentModel = "";
        models = [];
        return;
      }

      const slashIdx = mainValue.indexOf("/");
      if (slashIdx > 0) {
        currentProvider = mainValue.slice(0, slashIdx);
        currentModel = mainValue.slice(slashIdx + 1);
      } else {
        currentProvider = "";
        currentModel = mainValue;
      }

      // Find provider config to fetch model list
      const provEntry = parsed.providers.find((p) => p.name === currentProvider);
      if (provEntry) {
        const result = await fetchModels(agent, provEntry.type, provEntry.apiKey, provEntry.url);
        if (load === loadCount) models = result.models;
      }
    } catch {
      // config not available yet
    }
  }

  async function selectModel(modelId: string): Promise<void> {
    if (modelId === currentModel || saving) return;
    saving = true;
    open = false;

    try {
      const agent = requireAgent(ws.agent);
      const provider = currentProvider;
      // The model is chosen from the main provider's list, so it goes with
      // that provider; the thinking level and temperature stay as they are.
      const saved = await configCoordinator.edit(agentConfigFile(agent, "providers"), (raw) => {
        const overrides = parseProvidersToml(raw).models.overrides.main;
        return { models: { main: modelRoleJson(provider + "/" + modelId, overrides) } };
      });
      if (!saved.result.valid) throw new Error(saved.result.error ?? "unknown error");
      ws.send({ type: "reload" });
      currentModel = modelId;
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't switch the model." }));
    } finally {
      saving = false;
    }
  }

  function toggle(): void {
    if (disabled) return;
    open = !open;
  }
</script>

<div
  class="model-selector-wrap"
  use:clickOutside={{
    onOutside: () => {
      open = false;
    },
  }}
>
  <button class="model-chip" onclick={toggle} disabled={disabled || saving} title="Switch model">
    {#if currentModel}
      <span class="model-chip-name">{currentModel}</span>
    {:else if settled}
      <span class="model-chip-name model-chip-empty">no model</span>
    {:else}
      <span class="model-chip-name model-chip-loading" aria-label="loading model">—</span>
    {/if}
    <span class="model-chip-chevron">{open ? "\u25B4" : "\u25BE"}</span>
  </button>

  {#if open && models.length > 0}
    <div class="model-dropdown" role="listbox">
      {#each models as model (model.id)}
        <div
          class="model-dropdown-item"
          class:active={model.id === currentModel}
          role="option"
          tabindex="-1"
          aria-selected={model.id === currentModel}
          onmousedown={(e: MouseEvent) => {
            e.preventDefault();
            void selectModel(model.id);
          }}
        >
          {model.name || model.id}
        </div>
      {/each}
    </div>
  {/if}
</div>
