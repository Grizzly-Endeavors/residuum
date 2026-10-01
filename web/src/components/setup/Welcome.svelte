<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import { agentNameProblem } from "../../lib/agent-name";
  import { TextField } from "../../lib/ui";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
  }

  let { wizardState = $bindable(), onNext }: Props = $props();

  let agentNameError = $derived(agentNameProblem(wizardState.agentName));
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
    hint="Lowercase letters, digits and hyphens. It's your agent's permanent name and folder."
    error={agentNameError ?? undefined}
  />
  <TextField
    label="Time zone"
    bind:value={wizardState.timezone}
    placeholder="America/New_York"
    autocapitalize="off"
    autocomplete="off"
    spellcheck="false"
    hint="An IANA time zone name, like America/New_York."
  />
</SetupGroup>

<SetupNav {onNext} nextDisabled={agentNameError !== null} />
