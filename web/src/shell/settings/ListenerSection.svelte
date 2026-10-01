<script lang="ts">
  import { TextField } from "../../lib/ui";
  import CallerKeysGroup from "./CallerKeysGroup.svelte";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import { fieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Advanced → Agent-to-agent for the install: the one listener that serves
  // every agent to the agents outside it, and the caller keys it accepts. The
  // listener's settings are staged and saved with the rest of the hub's
  // `config.toml`. Caller keys act at once. Each agent's own visibility is its
  // own section's setting.

  let { scope, section }: AllSectionProps = $props();

  const on = $derived(scope.config.a2a_enabled);
</script>

<SettingsSection
  {scope}
  {section}
  title="Agent-to-agent"
  lede="Let agents elsewhere find yours and hand them work. One listener serves every agent on this install; each agent's own visibility is set in its settings."
>
  <SettingsGroup title="Listener">
    <ConfigToggle
      {scope}
      field="a2a_enabled"
      label="Let other agents reach this install"
      hint="Off: nothing outside this install can reach its agents over A2A. On: other agents can find them, and anyone with a caller key can hand them work. Saving restarts the listener."
    />
    <ConfigNumber
      {scope}
      field="a2a_port"
      label="Listener port"
      placeholder="7702"
      min={1}
      max={65535}
      disabled={!on}
      hint="Leave it empty to use 7702. If you run your own tunnel, expose only this port through it."
    />
    <TextField
      label="Your own address"
      bind:value={scope.config.a2a_public_url}
      disabled={!on}
      placeholder="https://your-own-tunnel.example/a2a"
      autocomplete="off"
      spellcheck={false}
      code
      hint="Only needed if you run your own tunnel: Residuum Cloud gives each agent its own address. Each agent is served under /agents/name at this address."
      error={fieldError(scope, { kind: "config", field: "a2a_public_url" })}
    />
  </SettingsGroup>

  <CallerKeysGroup />
</SettingsSection>
