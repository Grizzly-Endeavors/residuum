<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { fetchProvidersRaw, putProvidersRaw } from "../lib/api";
  import { parseProvidersToml, serializeProvidersToml } from "../lib/settings-toml";
  import { withConfigLock } from "../lib/config-lock";

  let { disabled = false }: { disabled?: boolean } = $props();

  const LEVELS = ["off", "low", "medium", "high"] as const;
  const LABELS: Record<string, string> = {
    off: "Off",
    low: "Low",
    medium: "Med",
    high: "High",
  };

  let currentLevel = $state("");
  let saving = $state(false);

  onMount(async () => {
    try {
      const raw = await fetchProvidersRaw();
      const parsed = parseProvidersToml(raw);
      currentLevel = parsed.models.overrides.main?.thinking ?? "";
    } catch {
      // config not available yet
    }
  });

  async function setLevel(level: string): Promise<void> {
    if (saving || disabled) return;

    // Toggle off if clicking active level
    const newLevel = level === currentLevel ? "" : level;
    saving = true;

    await withConfigLock(async () => {
      try {
        const raw = await fetchProvidersRaw();
        const parsed = parseProvidersToml(raw);
        parsed.models.overrides.main ??= { temperature: "", thinking: "" };
        parsed.models.overrides.main.thinking = newLevel;
        const toml = serializeProvidersToml(parsed.providers, parsed.models);
        await putProvidersRaw(toml);
        ws.send({ type: "reload" });
        currentLevel = newLevel;
      } catch {
        // save failed
      }
    });

    saving = false;
  }
</script>

<div class="thinking-compact">
  {#each LEVELS as level (level)}
    <button
      class="seg-btn"
      class:active={level === currentLevel}
      disabled={disabled || saving}
      title="Thinking: {level}"
      onmousedown={(e: MouseEvent) => {
        e.preventDefault();
        void setLevel(level);
      }}
    >
      {LABELS[level]}
    </button>
  {/each}
</div>
