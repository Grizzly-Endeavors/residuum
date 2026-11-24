<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { SetupWizardState, McpCatalogEntry, ProviderKey } from "./lib/types";
  import { fetchTimezone, fetchMcpCatalog } from "./lib/api";
  import { DEFAULT_AGENT_NAME } from "./lib/agent-name";
  import { userErrorMessage } from "./lib/errors";
  import { markCatalogKeys } from "./lib/setup-keys";
  import { Icon } from "./lib/icons";
  import { Banner } from "./lib/ui";
  import SetupProgress from "./components/setup/SetupProgress.svelte";
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

  const STEPS = [
    {
      label: "Welcome",
      title: "Welcome to Residuum",
      lede: "Set up your first agent. It takes about a minute.",
    },
    {
      label: "Providers",
      title: "Add model providers",
      lede: "Choose at least one. You can mix providers across roles, and add more later in Settings.",
    },
    {
      label: "Models",
      title: "Assign models",
      lede: "Choose the model each role uses, or keep the defaults.",
    },
    {
      label: "Tool servers",
      title: "Add tool servers",
      lede: "Tool servers (MCP) give your agent more tools. They're optional, and you can add more later in Settings.",
    },
    {
      label: "Connections",
      title: "Connect chat apps",
      lede: "Talk to your agent from Discord, Telegram or Microsoft Teams. These are optional, and you can add them later in Settings.",
    },
    {
      label: "Save",
      title: "Save and start",
      lede: "Check your choices, then save to start Residuum.",
    },
  ] as const;
  const TOTAL_STEPS = STEPS.length;
  const stepLabels = STEPS.map((s) => s.label);

  const STORAGE_KEY = "residuum-setup-draft";

  interface PersistedWizard {
    step: number;
    wizardState: SetupWizardState;
  }

  function defaultWizardState(): SetupWizardState {
    return {
      userName: "",
      agentName: DEFAULT_AGENT_NAME,
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
    clone.mcpServers = clone.mcpServers.map((srv) => {
      if (!srv.secretEnvKeys || srv.secretEnvKeys.length === 0) return srv;
      const env = { ...srv.env };
      for (const field of srv.secretEnvKeys) env[field] = "";
      return { ...srv, env };
    });
    return clone;
  }

  const persisted = loadPersisted();

  let step = $state(Math.min(Math.max(persisted?.step ?? 0, 0), TOTAL_STEPS - 1));
  let catalog = $state<McpCatalogEntry[]>([]);
  let catalogLoading = $state(false);
  let catalogError = $state<string | null>(null);

  let showDraftKeyNotice = $state(persisted !== null);

  let wizardState = $state<SetupWizardState>({
    ...defaultWizardState(),
    ...persisted?.wizardState,
  });

  async function loadCatalog() {
    catalogLoading = true;
    catalogError = null;
    try {
      catalog = await fetchMcpCatalog();
      markCatalogKeys(wizardState.mcpServers, catalog);
    } catch (err: unknown) {
      catalogError = userErrorMessage(err, { action: "Couldn't load the MCP server catalog." });
    } finally {
      catalogLoading = false;
    }
  }

  onMount(async () => {
    const tz = await fetchTimezone();
    if (!wizardState.timezone) wizardState.timezone = tz;
    void loadCatalog();
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
    return () => clearTimeout(persistTimer);
  });

  function clearPersisted() {
    if (persistTimer) clearTimeout(persistTimer);
    localStorage.removeItem(STORAGE_KEY);
  }

  function handleComplete() {
    clearPersisted();
    onComplete();
  }

  let scroller = $state<HTMLElement>();
  let heading = $state<HTMLHeadingElement>();

  async function goTo(index: number) {
    step = index;
    await tick();
    if (scroller) scroller.scrollTop = 0;
    heading?.focus();
  }

  function next() {
    if (step < TOTAL_STEPS - 1) void goTo(step + 1);
  }

  function back() {
    if (step > 0) void goTo(step - 1);
  }

  const current = $derived(STEPS[step] ?? STEPS[0]);
</script>

<div class="setup-wizard">
  <header class="setup-bar">
    <span class="setup-wordmark"><Icon name="mark" size={18} />Residuum</span>
  </header>
  <main class="setup-scroller" bind:this={scroller}>
    <div class="setup-column" data-setup-column>
      <SetupProgress labels={stepLabels} current={step} />

      <div class="setup-head">
        <h1 class="setup-title" tabindex="-1" bind:this={heading}>{current.title}</h1>
        <p class="setup-lede">{current.lede}</p>
      </div>

      {#if showDraftKeyNotice}
        <Banner tone="info" ondismiss={() => (showDraftKeyNotice = false)}>
          Picked up where you left off. Keys and tokens aren't kept in the draft, so enter them
          again on the Providers, Tool servers and Connections steps before you finish.
        </Banner>
      {/if}

      {#if step === 0}
        <Welcome bind:wizardState onNext={next} />
      {:else if step === 1}
        <Providers
          bind:wizardState
          onNext={next}
          onBack={back}
          restoredDraft={persisted !== null}
        />
      {:else if step === 2}
        <Roles bind:wizardState onNext={next} onBack={back} />
      {:else if step === 3}
        <MCP
          bind:wizardState
          {catalog}
          {catalogLoading}
          {catalogError}
          onRetryCatalog={loadCatalog}
          onNext={next}
          onBack={back}
        />
      {:else if step === 4}
        <Integrations bind:wizardState onNext={next} onBack={back} />
      {:else if step === 5}
        <Review bind:wizardState onBack={back} onComplete={handleComplete} />
      {/if}
    </div>
  </main>
</div>

<style>
  .setup-wizard {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
  }

  .setup-bar {
    display: flex;
    flex: none;
    align-items: center;
    height: var(--layout-place-header-height);
    padding: 0 var(--space-20);
    border-bottom: 1px solid var(--color-line-soft);
  }

  .setup-wordmark {
    display: inline-flex;
    align-items: center;
    gap: var(--space-10);
    font-family: var(--font-mark);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    letter-spacing: 0.2em;
    text-transform: uppercase;

    & > :global(svg) {
      color: var(--color-vein);
    }
  }

  .setup-scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .setup-column {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    max-width: calc(640px + 2 * var(--space-16));
    margin: 0 auto;
    padding: var(--space-32) var(--space-16) var(--space-48);
  }

  .setup-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    margin: var(--space-16) 0 var(--space-8);
  }

  .setup-title {
    font-size: var(--font-size-title);
    font-weight: var(--font-weight-semibold);
    --leading: var(--line-height-tight);

    &:focus {
      outline: none;
    }
  }

  .setup-lede {
    color: var(--color-text-2);
  }

  @media (max-width: 760px) {
    .setup-bar {
      padding: 0 var(--space-16);
    }

    .setup-column {
      padding-top: var(--space-20);
    }
  }
</style>
