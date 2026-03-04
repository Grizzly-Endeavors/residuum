<script lang="ts">
  import type { SetupWizardState, ProviderKey } from "../../lib/types";
  import { EMBEDDING_PROVIDERS } from "../../lib/models";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState, onNext, onBack }: Props = $props();

  const providers: Record<
    ProviderKey,
    { name: string; desc: string; keyEnv: string; showUrl?: boolean }
  > = {
    anthropic: {
      name: "Anthropic",
      desc: "Claude models (Sonnet, Haiku, Opus)",
      keyEnv: "ANTHROPIC_API_KEY",
    },
    openai: {
      name: "OpenAI",
      desc: "OpenAI or compatible APIs (vLLM, LM Studio, etc.)",
      keyEnv: "OPENAI_API_KEY",
      showUrl: true,
    },
    gemini: {
      name: "Google Gemini",
      desc: "Gemini models via Google AI",
      keyEnv: "GEMINI_API_KEY",
    },
    ollama: {
      name: "Ollama",
      desc: "Local models (no API key needed)",
      keyEnv: "",
    },
  };

  const providerKeys: ProviderKey[] = ["anthropic", "openai", "gemini", "ollama"];

  function toggleProvider(key: ProviderKey) {
    const idx = wizardState.selectedProviders.indexOf(key);
    if (idx >= 0) {
      if (wizardState.selectedProviders.length <= 1) return;
      wizardState.selectedProviders.splice(idx, 1);
      if (wizardState.mainProvider === key) {
        wizardState.mainProvider = wizardState.selectedProviders[0] ?? "anthropic";
      }
    } else {
      wizardState.selectedProviders.push(key);
    }
  }

  let hasEmbeddingProvider = $derived(
    wizardState.selectedProviders.some((p) => EMBEDDING_PROVIDERS.includes(p)),
  );
</script>

<h2>Add Providers</h2>
<p class="subtitle">Select one or more LLM providers. You can mix providers across roles.</p>

<div>
  {#each providerKeys as key (key)}
    {@const p = providers[key]}
    {@const isSelected = wizardState.selectedProviders.includes(key)}
    <div
      class="provider-option"
      class:selected={isSelected}
      onclick={() => toggleProvider(key)}
      role="button"
      tabindex="0"
      onkeydown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          toggleProvider(key);
        }
      }}
    >
      <div class="provider-check">{isSelected ? "\u2713" : ""}</div>
      <div>
        <div class="provider-name">{p.name}</div>
        <div class="provider-desc">{p.desc}</div>
      </div>
    </div>
  {/each}
</div>

{#if !hasEmbeddingProvider && wizardState.selectedProviders.length > 0}
  <div class="provider-warning">
    <span class="provider-warning-icon">&#9888;</span>
    <span
      >None of the selected providers offer an embedding API. Memory search works best with
      embeddings — consider adding OpenAI, Gemini, or Ollama.</span
    >
  </div>
{/if}

{#if wizardState.selectedProviders.length > 0}
  <div class="provider-configs">
    {#each wizardState.selectedProviders as key (key)}
      {@const p = providers[key]}
      {@const cfg = wizardState.providerConfigs[key]}
      <div class="provider-config-section">
        <div class="provider-config-header">{p.name}</div>
        {#if key !== "ollama"}
          <div class="settings-field">
            <label for="prov-{key}-apikey"
              >API Key{p.keyEnv ? ` (or set ${p.keyEnv} env var)` : ""}</label
            >
            <input
              id="prov-{key}-apikey"
              type="password"
              bind:value={cfg.apiKey}
              placeholder="sk-..."
            />
          </div>
        {/if}
        {#if p.showUrl}
          <div class="settings-field">
            <label for="prov-{key}-url">Base URL (leave blank for default)</label>
            <input
              id="prov-{key}-url"
              type="text"
              bind:value={cfg.url}
              placeholder="https://api.openai.com/v1"
            />
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack}>Back</button>
  <span class="setup-nav-hint">You can add more providers later in settings.</span>
  <button class="btn btn-primary" onclick={onNext}>Next</button>
</div>
