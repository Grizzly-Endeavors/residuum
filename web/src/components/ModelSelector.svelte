<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { fetchProvidersRaw, putProvidersRaw } from "../lib/api";
  import { parseProvidersToml, serializeProvidersToml } from "../lib/settings-toml";
  import { fetchModels, type ModelEntry } from "../lib/models";
  import { withConfigLock } from "../lib/config-lock";
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

  async function loadCurrentModel(): Promise<void> {
    try {
      const raw = await fetchProvidersRaw();
      const parsed = parseProvidersToml(raw);
      const mainValue = parsed.models.main;
      if (!mainValue) return;

      const slashIdx = mainValue.indexOf("/");
      if (slashIdx > 0) {
        currentProvider = mainValue.slice(0, slashIdx);
        currentModel = mainValue.slice(slashIdx + 1);
      } else {
        currentModel = mainValue;
      }

      // Find provider config to fetch model list
      const provEntry = parsed.providers.find((p) => p.name === currentProvider);
      if (provEntry) {
        const result = await fetchModels(provEntry.type, provEntry.apiKey, provEntry.url);
        models = result.models;
      }
    } catch {
      // config not available yet
    }
  }

  async function selectModel(modelId: string): Promise<void> {
    if (modelId === currentModel || saving) return;
    saving = true;
    open = false;

    await withConfigLock(async () => {
      try {
        const raw = await fetchProvidersRaw();
        const parsed = parseProvidersToml(raw);
        parsed.models.main = currentProvider + "/" + modelId;
        const toml = serializeProvidersToml(parsed.providers, parsed.models);
        await putProvidersRaw(toml);
        ws.send({ type: "reload" });
        currentModel = modelId;
      } catch {
        // save failed — keep previous state
      }
    });

    saving = false;
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
