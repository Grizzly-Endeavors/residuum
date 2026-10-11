<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import { agentNameProblem } from "../../lib/agent-name";
  import { isTimeZoneName, timeZoneChoices } from "../../lib/time-zones";
  import { SelectField, TextField } from "../../lib/ui";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
  }

  let { wizardState = $bindable(), onNext }: Props = $props();

  let agentNameError = $derived(agentNameProblem(wizardState.agentName));
  const zone = $derived(wizardState.timezone.trim());
  const zoneChoices = $derived(timeZoneChoices(wizardState.timezone));
  const zoneError = $derived(
    zone !== "" && !isTimeZoneName(zone) ? "That doesn't look like a time zone name." : undefined,
  );
</script>

<SetupGroup>
  <TextField
    label="Your name"
    bind:value={wizardState.userName}
    autocomplete="name"
    placeholder="What should your agent call you?"
    hint="Optional. Your agents read it from the team's shared notes about you."
  />
  <TextField
    label="Agent name"
    bind:value={wizardState.agentName}
    placeholder="assistant"
    autocapitalize="off"
    autocomplete="off"
    spellcheck="false"
    hint="Up to 32 characters. Capitals, spaces, and letters from any language are fine."
    error={agentNameError ?? undefined}
  />
  <SelectField
    label="Time zone"
    bind:value={wizardState.timezone}
    options={zoneChoices.ungrouped}
    groups={zoneChoices.groups}
    placeholder="Choose a time zone"
    hint="Schedules and timestamps use this."
    error={zoneError}
  />
</SetupGroup>

<SetupNav
  {onNext}
  nextDisabled={agentNameError !== null || zone === "" || zoneError !== undefined}
/>
