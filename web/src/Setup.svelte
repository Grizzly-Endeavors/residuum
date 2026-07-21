<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import type { SetupWizardState, McpCatalogEntry, ProviderKey } from "./lib/types";
  import { fetchTimezone, fetchMcpCatalog } from "./lib/api";
  import Welcome from "./components/setup/Welcome.svelte";
  import Providers from "./components/setup/Providers.svelte";
  import Roles from "./components/setup/Roles.svelte";
  import MCP from "./components/setup/MCP.svelte";
  import Integrations from "./components/setup/Integrations.svelte";
  import Review from "./components/setup/Review.svelte";

  interface Props {
    onComplete: () => void;
  }

  let { onComplete }: Props = $props();

  const TOTAL_STEPS = 6;
  const stepIndices = Array.from({ length: TOTAL_STEPS }, (_, i) => i);
  let step = $state(0);
  let catalog = $state<McpCatalogEntry[]>([]);
  let completed = false;

  let wizardState = $state<SetupWizardState>({
    userName: "",
    timezone: "",
    selectedProviders: ["anthropic"] as ProviderKey[],
    providerConfigs: {
      anthropic: { apiKey: "", model: "", url: "" },
      openai: { apiKey: "", model: "", url: "" },
      gemini: { apiKey: "", model: "", url: "" },
      ollama: { apiKey: "", model: "", url: "" },
    },
    mainProvider: "anthropic",
    roles: {
      observer: { provider: "", url: "", model: "" },
      reflector: { provider: "", url: "", model: "" },
      pulse: { provider: "", url: "", model: "" },
    },
    embeddingModel: { provider: "", model: "" },
    backgroundModels: {
      small: { provider: "", model: "" },
      medium: { provider: "", model: "" },
      large: { provider: "", model: "" },
    },
    mcpServers: [],
    integrations: { discordToken: "", telegramToken: "" },
    secretRefs: {},
  });

  // Warn before an accidental reload/close discards in-progress setup input (provider
  // selections, role assignments, MCP servers, integration tokens). Not persisted to
  // storage on purpose: early steps hold raw API keys in memory before they're
  // exchanged for `secret:` references in Review.svelte, and storage would leave
  // plaintext keys sitting in the browser.
  function handleBeforeUnload(event: BeforeUnloadEvent) {
    if (step > 0 && !completed) {
      event.preventDefault();
    }
  }

  function handleComplete() {
    completed = true;
    onComplete();
  }

  onMount(async () => {
    window.addEventListener("beforeunload", handleBeforeUnload);
    const [tz, cat] = await Promise.all([fetchTimezone(), fetchMcpCatalog()]);
    wizardState.timezone = tz;
    catalog = cat;
  });

  onDestroy(() => {
    window.removeEventListener("beforeunload", handleBeforeUnload);
  });

  function next() {
    if (step < TOTAL_STEPS - 1) step++;
  }

  function back() {
    if (step > 0) step--;
  }
</script>

<div class="setup-view emerges">
  <div class="setup-body">
    <div class="setup-card">
      <div class="setup-step-indicator">
        {#each stepIndices as i (i)}
          <div class="step-dot" class:active={i === step} class:done={i < step}></div>
        {/each}
      </div>

      {#if step === 0}
        <Welcome {wizardState} onNext={next} />
      {:else if step === 1}
        <Providers {wizardState} onNext={next} onBack={back} />
      {:else if step === 2}
        <Roles {wizardState} onNext={next} onBack={back} />
      {:else if step === 3}
        <MCP {wizardState} {catalog} onNext={next} onBack={back} />
      {:else if step === 4}
        <Integrations {wizardState} onNext={next} onBack={back} />
      {:else if step === 5}
        <Review {wizardState} onBack={back} onComplete={handleComplete} />
      {/if}
    </div>
  </div>
</div>
