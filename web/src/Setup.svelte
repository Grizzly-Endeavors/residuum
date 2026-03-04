<script lang="ts">
  import { onMount } from "svelte";
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
  let step = $state(0);
  let catalog = $state<McpCatalogEntry[]>([]);

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
      observer: { provider: "", apiKey: "", url: "", model: "" },
      reflector: { provider: "", apiKey: "", url: "", model: "" },
      pulse: { provider: "", apiKey: "", url: "", model: "" },
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

  onMount(async () => {
    const [tz, cat] = await Promise.all([fetchTimezone(), fetchMcpCatalog()]);
    wizardState.timezone = tz;
    catalog = cat;
  });

  function next() {
    if (step < TOTAL_STEPS - 1) step++;
  }

  function back() {
    if (step > 0) step--;
  }
</script>

<div class="setup-view">
  <div class="setup-body">
    <div class="setup-card">
      <div class="setup-step-indicator">
        {#each Array(TOTAL_STEPS) as _, i (i)}
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
        <Review {wizardState} onBack={back} {onComplete} />
      {/if}
    </div>
  </div>
</div>
