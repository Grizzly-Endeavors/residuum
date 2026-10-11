<script lang="ts">
  import type { SetupWizardState, ProviderKey } from "../../lib/types";
  import { EMBEDDING_PROVIDERS } from "../../lib/models";
  import { Banner, SecretField, TextField, Toggle } from "../../lib/ui";
  import { PROVIDER_KEYS, PROVIDER_NAMES, setProviderSelected } from "./providers";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState = $bindable(), onNext, onBack }: Props = $props();

  const details: Record<ProviderKey, { desc: string; keyEnv?: string; keyPlaceholder?: string }> = {
    anthropic: { desc: "Claude models: Sonnet, Haiku and Opus.", keyEnv: "ANTHROPIC_API_KEY" },
    openai: {
      desc: "OpenAI, or an API compatible with it, such as vLLM or LM Studio.",
      keyEnv: "OPENAI_API_KEY",
    },
    gemini: { desc: "Gemini models through Google AI.", keyEnv: "GEMINI_API_KEY" },
    fireworks: {
      desc: "Open models such as GLM, Kimi, DeepSeek and Qwen, hosted by Fireworks.",
      keyEnv: "FIREWORKS_API_KEY",
      keyPlaceholder: "fw_...",
    },
    ollama: { desc: "Models running on your own machine. No API key needed." },
  };

  const NO_SECRET = { kind: "none" } as const;

  let hasEmbeddingProvider = $derived(
    wizardState.selectedProviders.some((p) => EMBEDDING_PROVIDERS.includes(p)),
  );
</script>

<SetupGroup>
  {#each PROVIDER_KEYS as key (key)}
    {@const p = details[key]}
    {@const isSelected = wizardState.selectedProviders.includes(key)}
    {@const cfg = wizardState.providerConfigs[key]}
    <div class="setup-provider">
      <Toggle
        label={PROVIDER_NAMES[key]}
        hint={p.desc}
        bind:checked={() => isSelected, (on) => setProviderSelected(wizardState, key, on)}
        disabled={isSelected && wizardState.selectedProviders.length === 1}
      />
      {#if isSelected && p.keyEnv !== undefined}
        <SecretField
          label="{PROVIDER_NAMES[key]} API key"
          source={NO_SECRET}
          bind:value={cfg.apiKey}
          placeholder={p.keyPlaceholder ?? "sk-..."}
          hint="Or set the {p.keyEnv} environment variable instead."
        />
      {/if}
      {#if isSelected && key === "openai"}
        <TextField
          label="Base URL"
          bind:value={cfg.url}
          placeholder="https://api.openai.com/v1"
          hint="Leave blank to use the default."
          autocapitalize="off"
          autocomplete="off"
          spellcheck="false"
        />
      {/if}
    </div>
  {/each}
</SetupGroup>

{#if !hasEmbeddingProvider}
  <Banner tone="info">
    None of these providers offers embeddings, which memory search works best with. Consider adding
    OpenAI, Gemini, Fireworks or Ollama.
  </Banner>
{/if}

<SetupNav {onBack} {onNext} />

<style>
  .setup-provider {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);

    & + & {
      padding-top: var(--space-16);
      border-top: 1px solid var(--color-line-soft);
    }
  }
</style>
