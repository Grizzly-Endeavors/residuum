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
  const stepIndices = Array.from({ length: TOTAL_STEPS }, (_, i) => i);

  // ── Draft persistence ────────────────────────────────────────────────
  // Setup is the highest-stakes form in the app (hand-typed API keys across
  // up to 6 steps) with no undo — a refresh or crashed tab must not throw
  // away everything the user just entered. We keep a draft in localStorage
  // and restore it on mount, but never persist raw provider API keys or
  // integration tokens (matching the convention in Review.svelte, which
  // only ever sends those to the backend via storeSecret(), never stores
  // them client-side as plain text).
  const STORAGE_KEY = "residuum-setup-draft";

  interface PersistedWizard {
    step: number;
    wizardState: SetupWizardState;
  }

  function defaultWizardState(): SetupWizardState {
    return {
      userName: "",
      timezone: "",
      selectedProviders: ["anthropic"] as ProviderKey[],
      providerConfigs: {
        anthropic: { apiKey: "", model: "", url: "" },
        openai: { apiKey: "", model: "", url: "" },
        gemini: { apiKey: "", model: "", url: "" },
        fireworks: { apiKey: "", model: "", url: "" },
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
      integrations: {
        discordToken: "",
        telegramToken: "",
        teamsAppId: "",
        teamsTenantId: "",
        teamsAppPassword: "",
      },
      secretRefs: {},
    };
  }

  function loadPersisted(): PersistedWizard | null {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw) as Partial<PersistedWizard>;
      if (typeof parsed.step !== "number" || typeof parsed.wizardState !== "object") {
        return null;
      }
      return { step: parsed.step, wizardState: parsed.wizardState };
    } catch (err: unknown) {
      // eslint-disable-next-line no-console -- draft is discarded before any UI mounts; console is the only channel
      console.warn("discarding unreadable setup draft", err);
      localStorage.removeItem(STORAGE_KEY);
      return null;
    }
  }

  // Strip fields that should never be written to localStorage in the
  // clear, even transiently. These are exactly the fields Review.svelte
  // exchanges for a secret reference before setup completes.
  function sanitizeForStorage(state: SetupWizardState): SetupWizardState {
    const clone = structuredClone(state);
    for (const key of Object.keys(clone.providerConfigs) as ProviderKey[]) {
      clone.providerConfigs[key] = { ...clone.providerConfigs[key], apiKey: "" };
    }
    clone.integrations = {
      discordToken: "",
      telegramToken: "",
      teamsAppId: clone.integrations.teamsAppId,
      teamsTenantId: clone.integrations.teamsTenantId,
      teamsAppPassword: "",
    };
    return clone;
  }

  const persisted = loadPersisted();

  let step = $state(Math.min(Math.max(persisted?.step ?? 0, 0), TOTAL_STEPS - 1));
  let catalog = $state<McpCatalogEntry[]>([]);

  let wizardState = $state<SetupWizardState>(persisted?.wizardState ?? defaultWizardState());

  onMount(async () => {
    const [tz, cat] = await Promise.all([fetchTimezone(), fetchMcpCatalog()]);
    // Don't clobber a timezone the user already resolved in a prior session.
    if (!wizardState.timezone) wizardState.timezone = tz;
    catalog = cat;
  });

  let persistTimer: ReturnType<typeof setTimeout> | undefined;

  function schedulePersist() {
    if (persistTimer) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      try {
        const sanitized = sanitizeForStorage($state.snapshot(wizardState));
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ step, wizardState: sanitized }));
      } catch (err: unknown) {
        // eslint-disable-next-line no-console -- debounced autosave; a toast here would fire on every keystroke
        console.warn("failed to persist setup draft", err);
      }
    }, 500);
  }

  $effect(() => {
    $state.snapshot(wizardState);
    step;
    schedulePersist();
  });

  function clearPersisted() {
    if (persistTimer) clearTimeout(persistTimer);
    localStorage.removeItem(STORAGE_KEY);
  }

  function handleComplete() {
    clearPersisted();
    onComplete();
  }

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
