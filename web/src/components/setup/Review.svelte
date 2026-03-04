<script lang="ts">
  import { onMount } from "svelte";
  import type { SetupWizardState } from "../../lib/types";
  import { generateToml } from "../../lib/toml";
  import { storeSecret, completeSetup } from "../../lib/api";

  interface Props {
    wizardState: SetupWizardState;
    onBack: () => void;
    onComplete: () => void;
  }

  let { wizardState, onBack, onComplete }: Props = $props();

  let tomlText = $state("");
  let saving = $state(false);
  let validationMsg = $state("");
  let validationClass = $state("");

  onMount(() => {
    tomlText = generateToml(state);
  });

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
      const prov = r.provider || wizardState.mainProvider;
      const provKey = wizardState.providerConfigs[prov as keyof typeof wizardState.providerConfigs]?.apiKey || "";
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

    // Regenerate TOML with secret references
    const finalToml = generateToml(state);

    try {
      const result = await completeSetup(finalToml);
      if (result.valid) {
        validationMsg = "Configuration saved! Starting gateway...";
        validationClass = "success";
        setTimeout(() => onComplete(), 1500);
      } else {
        validationMsg = result.error || "Validation failed";
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

<h2>Review Configuration</h2>
<p class="subtitle">Here's your generated config. Edit if needed, then save to start Residuum.</p>

<textarea class="toml-editor" bind:value={tomlText}></textarea>

{#if validationMsg}
  <div class="validation-msg {validationClass}">{validationMsg}</div>
{/if}

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack} disabled={saving}>Back</button>
  <button class="btn btn-primary" onclick={handleSave} disabled={saving}>
    {saving ? "Saving..." : "Save & Start"}
  </button>
</div>
