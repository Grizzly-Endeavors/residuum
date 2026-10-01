<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import {
    generateHubConfigToml,
    generateConfigToml,
    generateProvidersToml,
    generateMcpJson,
  } from "../../lib/toml";
  import { storeSecret, completeSetup } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { Banner } from "../../lib/ui";
  import { providerName } from "./providers";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onBack: () => void;
    onComplete: () => void;
  }

  let { wizardState = $bindable(), onBack, onComplete }: Props = $props();

  let saving = $state(false);
  let outcome = $state<{ tone: "saved" | "error"; message: string } | null>(null);

  const teamsComplete = $derived(
    Boolean(
      wizardState.integrations.teamsAppId &&
      wizardState.integrations.teamsTenantId &&
      wizardState.integrations.teamsAppPassword,
    ),
  );

  const connections = $derived(
    [
      wizardState.integrations.discordToken ? "Discord" : "",
      wizardState.integrations.telegramToken ? "Telegram" : "",
      teamsComplete ? "Microsoft Teams" : "",
    ].filter(Boolean),
  );

  async function storeAllSecrets(): Promise<void> {
    wizardState.secretRefs = {};
    const promises: Promise<void>[] = [];

    // Provider API keys
    for (const prov of wizardState.selectedProviders) {
      const cfg = wizardState.providerConfigs[prov];
      if (prov !== "ollama" && cfg.apiKey) {
        promises.push(
          storeSecret(prov, cfg.apiKey).then((res) => {
            wizardState.secretRefs[prov] = res.reference;
          }),
        );
      }
    }

    // Integration tokens
    if (wizardState.integrations.discordToken) {
      promises.push(
        storeSecret("discord", wizardState.integrations.discordToken).then((res) => {
          wizardState.secretRefs["discord"] = res.reference;
        }),
      );
    }
    if (wizardState.integrations.telegramToken) {
      promises.push(
        storeSecret("telegram", wizardState.integrations.telegramToken).then((res) => {
          wizardState.secretRefs["telegram"] = res.reference;
        }),
      );
    }
    if (teamsComplete) {
      promises.push(
        storeSecret("teams", wizardState.integrations.teamsAppPassword).then((res) => {
          wizardState.secretRefs["teams"] = res.reference;
        }),
      );
    }

    await Promise.all(promises);
  }

  async function handleSave() {
    saving = true;
    outcome = null;

    try {
      await storeAllSecrets();
    } catch (err: unknown) {
      outcome = {
        tone: "error",
        message: userErrorMessage(err, { action: "Couldn't store your keys and tokens." }),
      };
      saving = false;
      return;
    }

    // Generate all config files with secret references
    const hubConfigToml = generateHubConfigToml(wizardState);
    const configToml = generateConfigToml(wizardState);
    const providersToml = generateProvidersToml(wizardState);
    const mcpJson = wizardState.mcpServers.length > 0 ? generateMcpJson(wizardState) : undefined;

    try {
      const result = await completeSetup({
        hubConfig: hubConfigToml,
        agentName: wizardState.agentName,
        userName: wizardState.userName,
        config: configToml,
        providers: providersToml,
        mcpJson,
      });
      if (result.valid) {
        outcome = { tone: "saved", message: `Saved. Starting ${wizardState.agentName}…` };
        setTimeout(() => onComplete(), 1500);
      } else {
        outcome = { tone: "error", message: result.error ?? "The configuration didn't validate." };
        saving = false;
      }
    } catch (err: unknown) {
      outcome = {
        tone: "error",
        message: userErrorMessage(err, { action: "Couldn't save the configuration." }),
      };
      saving = false;
    }
  }
</script>

<SetupGroup>
  <dl class="setup-summary">
    {#if wizardState.userName.trim() !== ""}
      <div class="setup-summary-row">
        <dt>Your name</dt>
        <dd>{wizardState.userName.trim()}</dd>
      </div>
    {/if}
    <div class="setup-summary-row">
      <dt>Agent</dt>
      <dd>{wizardState.agentName}</dd>
    </div>
    <div class="setup-summary-row">
      <dt>Providers</dt>
      <dd>{wizardState.selectedProviders.map(providerName).join(", ")}</dd>
    </div>
    <div class="setup-summary-row">
      <dt>Main model</dt>
      <dd>
        <code
          >{wizardState.mainProvider}/{wizardState.providerConfigs[wizardState.mainProvider]
            .model || "default"}</code
        >
      </dd>
    </div>
    {#if wizardState.mcpServers.length > 0}
      <div class="setup-summary-row">
        <dt>Tool servers</dt>
        <dd>{wizardState.mcpServers.map((s) => s.name).join(", ")}</dd>
      </div>
    {/if}
    {#if connections.length > 0}
      <div class="setup-summary-row">
        <dt>Connections</dt>
        <dd>{connections.join(", ")}</dd>
      </div>
    {/if}
  </dl>
</SetupGroup>

{#if outcome?.tone === "saved"}
  <Banner tone="info" icon="check">{outcome.message}</Banner>
{:else if outcome?.tone === "error"}
  <Banner tone="error">{outcome.message}</Banner>
{/if}

<SetupNav
  {onBack}
  backDisabled={saving}
  nextLabel="Save and start"
  onNext={() => void handleSave()}
  busy={saving}
/>

<style>
  .setup-summary {
    display: flex;
    flex-direction: column;
  }

  .setup-summary-row {
    display: grid;
    grid-template-columns: 9rem minmax(0, 1fr);
    gap: var(--space-4) var(--space-16);
    padding: var(--space-10) 0;

    &:first-child {
      padding-top: 0;
    }

    &:last-child {
      padding-bottom: 0;
    }

    & + & {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  dt {
    color: var(--color-text-2);
  }

  dd {
    overflow-wrap: anywhere;
  }

  @container setup-group (max-width: 400px) {
    .setup-summary-row {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
