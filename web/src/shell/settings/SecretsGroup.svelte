<script lang="ts">
  import { onMount, tick } from "svelte";
  import { deleteSecret, fetchSecretNames, storeSecret } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { toast } from "../../lib/toast.svelte";
  import {
    Banner,
    Button,
    confirmations,
    EmptyState,
    IconButton,
    SecretField,
    Skeleton,
    TextField,
  } from "../../lib/ui";
  import KeyForm from "./KeyForm.svelte";
  import KeyList from "./KeyList.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // The values settings refer to by name (`secret:openai`): provider keys,
  // channel tokens. Adding and removing act at once through the hub's own
  // endpoints and have no part in the save bar. Removing asks first, because
  // the hub keeps no copy to bring a value back from here; History can restore
  // the store as a whole. The list never has a value, and says nothing about
  // where a name is used, since the hub doesn't either.

  let names = $state.raw<string[] | null>(null);
  let loadError = $state("");
  let removing = $state<string | null>(null);

  let adding = $state(false);
  let saving = $state(false);
  let name = $state("");
  let value = $state("");
  let nameBox = $state<HTMLInputElement | HTMLTextAreaElement>();
  let addButton = $state<HTMLButtonElement>();

  const trimmed = $derived(name.trim());
  const replacing = $derived(names?.includes(trimmed) === true);
  const rows = $derived((names ?? []).map((each) => ({ name: each })));

  async function load(): Promise<void> {
    try {
      names = await fetchSecretNames();
      loadError = "";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the stored secrets." });
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
    await tick();
    addButton?.focus();
  }

  async function save(): Promise<void> {
    if (trimmed === "" || value === "" || saving) return;
    saving = true;
    try {
      await storeSecret(trimmed, value);
      toast.success(`Saved ${trimmed}.`);
      await close();
      await load();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't save ${trimmed}.` }));
    } finally {
      saving = false;
    }
  }

  async function remove(secret: string): Promise<void> {
    const confirmed = await confirmations.ask({
      title: `Remove ${secret}?`,
      message:
        "Anything that refers to it stops working until you store it again. Its value can't be recovered from here.",
      confirmLabel: "Remove secret",
      tone: "danger",
    });
    if (!confirmed) return;
    removing = secret;
    try {
      await deleteSecret(secret);
      toast.success(`Removed ${secret}.`);
      await load();
      // The row that held focus is gone.
      await tick();
      addButton?.focus();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't remove ${secret}.` }));
    } finally {
      removing = null;
    }
  }

  onMount(() => {
    void load();
  });
</script>

{#snippet removeAction(secret: { name: string })}
  <IconButton
    icon="trash"
    label="Remove {secret.name}"
    loading={removing === secret.name}
    onclick={() => void remove(secret.name)}
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
    Add a secret
  </Button>
{/snippet}

<SettingsGroup
  title="Stored secrets"
  lede="Values that settings refer to by name, such as provider API keys and channel tokens. Every agent on this install can use them. They are stored encrypted and can't be viewed after saving."
  foot={adding ? undefined : addAction}
>
  {#if loadError !== ""}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else if names === null}
    <Skeleton lines={2} label="Loading the stored secrets" />
  {:else if names.length === 0}
    <EmptyState>No secrets stored yet.</EmptyState>
  {:else}
    <KeyList label="Stored secrets" items={rows} action={removeAction} />
  {/if}

  {#if adding}
    <KeyForm
      label="Add a secret"
      submitLabel="Save secret"
      {saving}
      ready={trimmed !== "" && value !== ""}
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
        placeholder="openai"
        hint={replacing ? "Saving replaces the stored secret with this name." : undefined}
      />
      <SecretField label="Value" source={{ kind: "none" }} bind:value />
    </KeyForm>
  {/if}
</SettingsGroup>
