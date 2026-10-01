<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { fetchProvidersRaw } from "../lib/api";
  import { agentConfigFile, configCoordinator } from "../lib/config-coordinator";
  import { requireAgent } from "../lib/paths";
  import { parseProvidersToml, modelRoleJson } from "../lib/settings-toml";
  import { toast } from "../lib/toast.svelte";
  import { userErrorMessage } from "../lib/errors";

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

  onMount(() => {
    void loadLevel();
  });

  // The control shows what providers.toml says, so it follows the file: a
  // change made in settings, by history restore, or outside this page reloads it.
  $effect(() => {
    const agent = ws.agent;
    if (agent === null) return;
    return configCoordinator.subscribe(agentConfigFile(agent, "providers"), () => {
      void loadLevel();
    });
  });

  /** Sequences loads, so a slow one can't overwrite what a later one found. */
  let loadCount = 0;

  async function loadLevel(): Promise<void> {
    const load = ++loadCount;
    try {
      const raw = await fetchProvidersRaw(requireAgent(ws.agent));
      const parsed = parseProvidersToml(raw);
      if (load === loadCount) currentLevel = parsed.models.overrides.main?.thinking ?? "";
    } catch {
      // config not available yet
    }
  }

  async function setLevel(level: string): Promise<void> {
    if (saving || disabled) return;

    // Toggle off if clicking active level
    const newLevel = level === currentLevel ? "" : level;
    saving = true;

    try {
      const agent = requireAgent(ws.agent);
      const saved = await configCoordinator.edit(agentConfigFile(agent, "providers"), (raw) => {
        const { models } = parseProvidersToml(raw);
        const overrides = {
          temperature: models.overrides.main?.temperature ?? "",
          thinking: newLevel,
        };
        return { models: { main: modelRoleJson(models.main, overrides, models.fallbacks.main) } };
      });
      if (!saved.result.valid) throw new Error(saved.result.error ?? "unknown error");
      ws.send({ type: "reload" });
      currentLevel = newLevel;
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't change the thinking level." }));
    } finally {
      saving = false;
    }
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
<select
  class="thinking-mobile-select"
  disabled={disabled || saving}
  onchange={(e: Event) => {
    const val = (e.target as HTMLSelectElement).value;
    void setLevel(val);
  }}
>
  {#each LEVELS as level (level)}
    <option value={level} selected={level === currentLevel}>
      {level === currentLevel ? `\u2713 ${LABELS[level]}` : LABELS[level]}
    </option>
  {/each}
</select>
