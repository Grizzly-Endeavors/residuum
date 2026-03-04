<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import { generateConfigToml, generateProvidersToml, generateMcpJson } from "../../lib/toml";
  import { storeSecret, completeSetup } from "../../lib/api";

  interface Props {
    wizardState: SetupWizardState;
    onBack: () => void;
    onComplete: () => void;
  }

  let { wizardState, onBack, onComplete }: Props = $props();

  let saving = $state(false);
  let validationMsg = $state("");
  let validationClass = $state("");

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

    // Role-specific API keys (if different from provider config)
    for (const role of ["observer", "reflector", "pulse"]) {
      const r = wizardState.roles[role];
      if (!r) continue;
      const prov = r.provider || wizardState.mainProvider;
      const provKey =
        wizardState.providerConfigs[prov as keyof typeof wizardState.providerConfigs]?.apiKey || "";
      if (r.apiKey && r.apiKey !== provKey) {
        const name = `${role}_${prov}`;
        promises.push(
          storeSecret(name, r.apiKey).then((res) => {
            wizardState.secretRefs[name] = res.reference;
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

    await Promise.all(promises);
  }

  async function handleSave() {
    saving = true;
    validationMsg = "";
    validationClass = "";

    try {
      await storeAllSecrets();
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      validationMsg = "Failed to store secrets: " + message;
      validationClass = "error";
      saving = false;
      return;
    }

    // Generate all config files with secret references
    const configToml = generateConfigToml(wizardState);
    const providersToml = generateProvidersToml(wizardState);
    const mcpJson = wizardState.mcpServers.length > 0 ? generateMcpJson(wizardState) : undefined;

    try {
      const result = await completeSetup(configToml, providersToml, mcpJson);
      if (result.valid) {
        validationMsg = "Configuration saved! Starting gateway...";
        validationClass = "success";
        setTimeout(() => onComplete(), 1500);
      } else {
        validationMsg = result.error ?? "Validation failed";
        validationClass = "error";
        saving = false;
      }
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      validationMsg = "Network error: " + message;
      validationClass = "error";
      saving = false;
    }
  }
</script>

<h2>Save & Start</h2>
<p class="subtitle">Your configuration is ready. Click below to save and start Residuum.</p>

<div class="review-summary">
  <div class="review-item">
    <span class="review-label">Providers</span>
    <span class="review-value">{wizardState.selectedProviders.join(", ")}</span>
  </div>
  <div class="review-item">
    <span class="review-label">Main model</span>
    <span class="review-value"
      >{wizardState.mainProvider}/{wizardState.providerConfigs[wizardState.mainProvider].model ||
        "default"}</span
    >
  </div>
  {#if wizardState.mcpServers.length > 0}
    <div class="review-item">
      <span class="review-label">MCP servers</span>
      <span class="review-value">{wizardState.mcpServers.map((s) => s.name).join(", ")}</span>
    </div>
  {/if}
  {#if wizardState.integrations.discordToken || wizardState.integrations.telegramToken}
    <div class="review-item">
      <span class="review-label">Integrations</span>
      <span class="review-value">
        {[
          wizardState.integrations.discordToken ? "Discord" : "",
          wizardState.integrations.telegramToken ? "Telegram" : "",
        ]
          .filter(Boolean)
          .join(", ")}
      </span>
    </div>
  {/if}
</div>

{#if validationMsg}
  <div class="validation-msg {validationClass}">{validationMsg}</div>
{/if}

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack} disabled={saving}>Back</button>
  <button class="btn btn-primary" onclick={handleSave} disabled={saving}>
    {saving ? "Saving..." : "Save & Start"}
  </button>
</div>
