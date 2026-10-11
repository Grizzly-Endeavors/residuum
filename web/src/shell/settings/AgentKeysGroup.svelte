<script lang="ts">
  import { onMount, tick } from "svelte";
  import { deleteAgentKey, fetchAgentKeys, storeAgentKey } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { toast } from "../../lib/toast.svelte";
  import type { AgentKeyInfo } from "../../lib/types";
  import { notifyWithUndo } from "../../lib/undo";
  import {
    Badge,
    Banner,
    Button,
    EmptyState,
    IconButton,
    SecretField,
    Skeleton,
    TextField,
  } from "../../lib/ui";
  import { isKeyName, KEY_NAME_RULE } from "./key-name";
  import KeyForm from "./KeyForm.svelte";
  import KeyList from "./KeyList.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // The keys an agent hands to the commands it runs, as environment
  // variables. Adding and removing act at once through the hub's own
  // endpoints and have no part in the save bar; a removal offers Undo from the
  // checkpoint the hub took just before it.

  /** Below this many characters, hiding a value from output by matching it is unreliable. It is a hint: the key still saves. */
  const SHORT_VALUE_LENGTH = 8;

  let keys = $state.raw<AgentKeyInfo[] | null>(null);
  let loadError = $state("");
  let removing = $state<string | null>(null);

  let adding = $state(false);
  let saving = $state(false);
  let name = $state("");
  let value = $state("");
  let description = $state("");
  let nameBox = $state<HTMLInputElement | HTMLTextAreaElement>();
  let addButton = $state<HTMLButtonElement>();

  const trimmed = $derived(name.trim());
  const nameValid = $derived(isKeyName(trimmed));
  const replacing = $derived(keys?.some((key) => key.name === trimmed) === true);
  const nameError = $derived(trimmed !== "" && !nameValid ? KEY_NAME_RULE : undefined);
  const nameHint = $derived.by(() => {
    if (nameError !== undefined) return undefined;
    if (!nameValid) return "Lowercase letters, digits and underscores.";
    const replaces = replacing ? " Saving replaces the existing key." : "";
    return `Commands that use it get $${trimmed.toUpperCase()}.${replaces}`;
  });
  const valueHint = $derived(
    value !== "" && value.length < SHORT_VALUE_LENGTH
      ? `A value under ${String(SHORT_VALUE_LENGTH)} characters can't be hidden from output reliably. It still saves.`
      : "Stored encrypted. It can't be viewed after saving.",
  );

  async function load(): Promise<void> {
    try {
      keys = await fetchAgentKeys();
      loadError = "";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the agent keys." });
    }
  }

  async function open(): Promise<void> {
    adding = true;
    await tick();
    nameBox?.focus();
  }

  async function close(): Promise<void> {
    adding = false;
    name = "";
    value = "";
    description = "";
    await tick();
    addButton?.focus();
  }

  async function save(): Promise<void> {
    if (!nameValid || value === "" || saving) return;
    saving = true;
    try {
      const saved = await storeAgentKey(trimmed, value, description.trim());
      toast.success(`Saved ${saved.name}. Commands that use it get $${saved.env_var}.`);
      if (saved.warning) toast.info(saved.warning);
      await close();
      await load();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't save ${trimmed}.` }));
    } finally {
      saving = false;
    }
  }

  async function remove(key: string): Promise<void> {
    removing = key;
    try {
      const checkpointId = await deleteAgentKey(key);
      notifyWithUndo(null, `Removed ${key}.`, "hub", "agent-keys.toml.enc", checkpointId, load);
      await load();
      // The row that held focus is gone.
      await tick();
      addButton?.focus();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't remove ${key}.` }));
    } finally {
      removing = null;
    }
  }

  onMount(() => {
    void load();
  });
</script>

{#snippet mark(key: AgentKeyInfo)}
  <code class="key-env">${key.env_var}</code>
  {#if key.created_by === "agent"}
    <Badge tone="positive" dot>Saved by the agent</Badge>
  {/if}
{/snippet}

{#snippet removeAction(key: AgentKeyInfo)}
  <IconButton
    icon="trash"
    label="Remove {key.name}"
    loading={removing === key.name}
    onclick={() => void remove(key.name)}
  />
{/snippet}

{#snippet addAction()}
  <Button
    variant="quiet"
    size="sm"
    icon="plus"
    bind:element={addButton}
    onclick={() => void open()}
  >
    Add a key
  </Button>
{/snippet}

<SettingsGroup
  title="Keys agents use"
  lede="Credentials an agent can hand to the commands it runs, as environment variables. It sees each key's name and description, never its value, and values are hidden from everything it reads back."
  foot={adding ? undefined : addAction}
>
  {#if loadError !== ""}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else if keys === null}
    <Skeleton lines={2} label="Loading the agent keys" />
  {:else if keys.length === 0}
    <EmptyState>No keys yet. Add one here, or ask the agent to save a token it creates.</EmptyState>
  {:else}
    <KeyList
      label="Agent keys"
      items={keys}
      {mark}
      note={(key) => key.description}
      action={removeAction}
    />
  {/if}

  {#if adding}
    <KeyForm
      label="Add an agent key"
      submitLabel="Save key"
      {saving}
      ready={nameValid && value !== ""}
      onsubmit={() => void save()}
      oncancel={() => void close()}
    >
      <TextField
        label="Name"
        bind:value={name}
        bind:element={nameBox}
        code
        autocomplete="off"
        spellcheck={false}
        placeholder="github_token"
        hint={nameHint}
        error={nameError}
      />
      <SecretField label="Value" source={{ kind: "none" }} bind:value hint={valueHint} />
      <TextField
        label="Description"
        bind:value={description}
        placeholder="GitHub token with read access to my repos"
        hint="What it's for and what it can reach. The agent reads this."
      />
    </KeyForm>
  {/if}
</SettingsGroup>

<style>
  .key-env {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
