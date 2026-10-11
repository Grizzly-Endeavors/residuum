<script lang="ts">
  import { MediaQuery } from "svelte/reactivity";
  import { nameIsTaken, newAgentNameProblem } from "../lib/agent-name";
  import { hub } from "../lib/hub.svelte";
  import {
    Button,
    Dialog,
    Disclosure,
    SelectField,
    Sheet,
    TextField,
    type Choice,
  } from "../lib/ui";
  import { PHONE_QUERY } from "../styles/breakpoints";

  // Create an agent: a name checked as it is typed, what the agent should help
  // with, and under More options whose model settings it copies and who can
  // find it. A dialog, or a sheet on phones. What was typed stays when it
  // closes without creating, and clears once the agent exists.

  interface Props {
    open: boolean;
    /** The agent exists and the dialog has closed. */
    oncreated?: (name: string) => void;
  }

  let { open = $bindable(false), oncreated }: Props = $props();

  const uid = $props.id();
  const phone = new MediaQuery(PHONE_QUERY);

  const TITLE = "Create an agent";
  const LEDE = "Give it a name and say what it should help with.";
  const VISIBILITY: readonly Choice[] = [
    { value: "private", label: "Private" },
    { value: "public", label: "Public" },
  ];
  const VISIBILITY_HINTS: Readonly<Record<string, string>> = {
    private: "Agents outside this install need a caller key even to see it.",
    public:
      "Anyone with its address can see what it does. Handing it work still needs a caller key.",
  };

  let name = $state("");
  let description = $state("");
  let modelsFrom = $state("");
  let visibility = $state("private");
  let moreOpen = $state(false);
  /** Create was pressed, so an empty name says so too. */
  let attempted = $state(false);
  let creating = $state(false);
  let nameInput = $state<HTMLInputElement | HTMLTextAreaElement>();

  const nameProblem = $derived(
    newAgentNameProblem(
      name,
      hub.agents.flatMap((agent) => [agent.display_name || agent.name, agent.name]),
    ),
  );
  const shownProblem = $derived(name !== "" || attempted ? nameProblem : null);
  const nameHint = $derived(
    hub.deleted.some((gone) => nameIsTaken(name, [gone.display_name || gone.name, gone.name]))
      ? `An agent called ${name.trim()} was deleted recently. To bring it back, restore it from Recently deleted on Home instead.`
      : "Up to 32 characters. Capitals, spaces, and letters from any language are fine.",
  );
  const modelChoices = $derived(
    hub.agents.map((agent) => ({
      value: agent.name,
      label: agent.display_name || agent.name,
    })),
  );
  const modelsProblem = $derived(
    attempted && modelsFrom === ""
      ? "There's no agent to copy model settings from. Reload the page to set one up from scratch."
      : undefined,
  );

  // The model settings come from an agent that exists: the first, until another is chosen.
  $effect(() => {
    if (!hub.agents.some((agent) => agent.name === modelsFrom)) {
      modelsFrom = hub.agents[0]?.name ?? "";
    }
  });

  function close(): void {
    open = false;
    attempted = false;
  }

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (creating) return;
    attempted = true;
    if (nameProblem !== null) {
      nameInput?.focus();
      return;
    }
    if (modelsFrom === "") {
      moreOpen = true;
      return;
    }
    creating = true;
    try {
      const role = description.trim();
      const agent = await hub.createAgent({
        name,
        description: role === "" ? null : role,
        models_from: modelsFrom,
        providers_toml: null,
        a2a_visibility: visibility === "public" ? "public" : "private",
      });
      // The hub store has said why it failed; what was typed stays for another try.
      if (agent === null) return;
      name = "";
      description = "";
      visibility = "private";
      moreOpen = false;
      close();
      oncreated?.(agent.name);
    } finally {
      creating = false;
    }
  }
</script>

{#snippet fields()}
  <form id="{uid}-form" class="create-form" novalidate onsubmit={create}>
    <TextField
      label="Name"
      bind:value={name}
      bind:element={nameInput}
      hint={nameHint}
      error={shownProblem ?? undefined}
      placeholder="research-buddy"
      autocomplete="off"
      autocapitalize="off"
      spellcheck={false}
      data-autofocus
    />
    <TextField
      label="What should it help with?"
      bind:value={description}
      multiline
      rows={3}
      hint="It writes its own notes and role page from this, and fills any gaps itself."
      placeholder="Keep track of my reading list and remind me what I haven't finished"
    />
    <Disclosure summary="More options" bind:open={moreOpen}>
      <div class="create-more">
        <SelectField
          label="Copy model settings from"
          bind:value={modelsFrom}
          options={modelChoices}
          disabled={modelChoices.length === 0}
          hint="It starts with this agent's providers and model choices."
          error={modelsProblem}
        />
        <SelectField
          label="Who can find it"
          bind:value={visibility}
          options={VISIBILITY}
          hint={VISIBILITY_HINTS[visibility]}
        />
      </div>
    </Disclosure>
  </form>
{/snippet}

{#snippet buttons()}
  <Button variant="quiet" onclick={close}>Cancel</Button>
  <Button variant="primary" type="submit" form="{uid}-form" loading={creating}>Create agent</Button>
{/snippet}

{#if phone.current}
  <Sheet bind:open title={TITLE} description={LEDE} onclose={close} actions={buttons}>
    {@render fields()}
  </Sheet>
{:else}
  <Dialog
    bind:open
    title={TITLE}
    description={LEDE}
    onclose={close}
    actions={buttons}
    children={fields}
  />
{/if}

<style>
  .create-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .create-more {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-14);
    width: 100%;
    padding-top: var(--space-10);
  }

  @media (max-width: 760px) {
    .create-more {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
