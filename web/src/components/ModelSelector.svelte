<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { fetchProvidersRaw, putProvidersRaw } from "../lib/api";
  import { parseProvidersToml, serializeProvidersToml } from "../lib/settings-toml";
  import { fetchModels, type ModelEntry } from "../lib/models";
  import { withConfigLock } from "../lib/config-lock";

  let { disabled = false }: { disabled?: boolean } = $props();

  let open = $state(false);
  let currentModel = $state("");
  let currentProvider = $state("");
  let models = $state<ModelEntry[]>([]);
  let saving = $state(false);
  let menuEl: HTMLDivElement | undefined = $state();

  onMount(async () => {
    await loadCurrentModel();
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

  // Click-outside handler
  $effect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent): void {
      if (menuEl && !menuEl.contains(e.target as Node)) {
        open = false;
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  });
</script>

<div class="model-selector-wrap" bind:this={menuEl}>
  <button class="model-chip" onclick={toggle} disabled={disabled || saving} title="Switch model">
    <span class="model-chip-name">{currentModel || "no model"}</span>
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
