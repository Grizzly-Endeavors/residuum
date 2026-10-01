<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { createA2aKey, fetchA2aKeys, revokeA2aKey } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { relativeTime } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import type { A2aKeyInfo } from "../../lib/types";
  import { notifyWithUndo } from "../../lib/undo";
  import { Banner, Button, EmptyState, IconButton, Skeleton, TextField } from "../../lib/ui";
  import { isKeyName, KEY_NAME_RULE } from "./key-name";
  import KeyForm from "./KeyForm.svelte";
  import KeyList from "./KeyList.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // The tokens other agents present to reach this install. Creating and
  // revoking act at once through the hub's own endpoints and have no part in
  // the save bar. A new key's token is in the response to creating it and
  // nowhere after, so it shows here until the user is done with it. A
  // revocation offers Undo from the checkpoint the hub took just before it.

  const COPIED_MS = 1800;

  let keys = $state.raw<A2aKeyInfo[] | null>(null);
  let loadError = $state("");
  let revoking = $state<string | null>(null);

  let adding = $state(false);
  let creating = $state(false);
  let name = $state("");
  let description = $state("");
  let nameBox = $state<HTMLInputElement | HTMLTextAreaElement>();
  let addButton = $state<HTMLButtonElement>();

  let minted = $state<{ name: string; token: string } | null>(null);
  let copied = $state(false);
  let copyButton = $state<HTMLButtonElement>();
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const trimmed = $derived(name.trim());
  const nameValid = $derived(isKeyName(trimmed));
  const exists = $derived(keys?.some((key) => key.name === trimmed) === true);
  const nameError = $derived.by(() => {
    if (trimmed !== "" && !nameValid) return KEY_NAME_RULE;
    return exists ? `A key named ${trimmed} already exists.` : undefined;
  });

  async function load(): Promise<void> {
    try {
      keys = await fetchA2aKeys();
      loadError = "";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the caller keys." });
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
    description = "";
    await tick();
    addButton?.focus();
  }

  async function create(): Promise<void> {
    creating = true;
    try {
      const made = await createA2aKey(trimmed, description.trim());
      minted = { name: made.name, token: made.token };
      adding = false;
      name = "";
      description = "";
      await load();
      await tick();
      copyButton?.focus();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't create ${trimmed}.` }));
    } finally {
      creating = false;
    }
  }

  async function copyToken(): Promise<void> {
    if (minted === null) return;
    try {
      await navigator.clipboard.writeText(minted.token);
    } catch {
      toast.error("Couldn't copy the key. Select it and copy it by hand.");
      return;
    }
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copied = false;
    }, COPIED_MS);
  }

  async function done(): Promise<void> {
    minted = null;
    copied = false;
    await tick();
    addButton?.focus();
  }

  async function revoke(key: string): Promise<void> {
    revoking = key;
    try {
      const checkpointId = await revokeA2aKey(key);
      notifyWithUndo(
        null,
        `Revoked ${key}. It can no longer reach your agents.`,
        "hub",
        "a2a-keys.toml",
        checkpointId,
        load,
      );
      await load();
      // The row that held focus is gone.
      await tick();
      addButton?.focus();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't revoke ${key}.` }));
    } finally {
      revoking = null;
    }
  }

  onMount(() => {
    void load();
  });
  onDestroy(() => {
    clearTimeout(copiedTimer);
  });

  function created(key: A2aKeyInfo): string {
    const ago = relativeTime(key.created_at);
    return ago === "" ? "" : `Created ${ago}`;
  }
</script>

{#snippet mark(key: A2aKeyInfo)}
  <span class="key-created">{created(key)}</span>
{/snippet}

{#snippet revokeAction(key: A2aKeyInfo)}
  <IconButton
    icon="trash"
    label="Revoke {key.name}"
    loading={revoking === key.name}
    onclick={() => void revoke(key.name)}
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
    Add a caller key
  </Button>
{/snippet}

<SettingsGroup
  title="Caller keys"
  lede="Tokens other agents present to reach yours. Give a key to each agent you want to let in; revoking it ends that agent's access right away."
  foot={adding ? undefined : addAction}
>
  {#if minted !== null}
    <Banner icon="key" title="Key for {minted.name} created.">
      Copy it now: you won't see it again. Give it to the agent as an
      <code>Authorization: Bearer</code> header.
      <code class="token">{minted.token}</code>
      {#snippet actions()}
        <Button
          size="sm"
          icon={copied ? "check" : "copy"}
          bind:element={copyButton}
          onclick={() => void copyToken()}
        >
          {copied ? "Copied" : "Copy key"}
        </Button>
        <Button size="sm" variant="quiet" onclick={() => void done()}>Done</Button>
      {/snippet}
    </Banner>
  {/if}

  {#if loadError !== ""}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else if keys === null}
    <Skeleton lines={2} label="Loading the caller keys" />
  {:else if keys.length === 0}
    <EmptyState>No caller keys yet. Add one for each agent you want to let in.</EmptyState>
  {:else}
    <KeyList
      label="Caller keys"
      items={keys}
      {mark}
      note={(key) => key.description}
      action={revokeAction}
    />
  {/if}

  {#if adding}
    <KeyForm
      label="Add a caller key"
      submitLabel="Create key"
      saving={creating}
      ready={nameValid && !exists}
      onsubmit={() => void create()}
      oncancel={() => void close()}
    >
      <TextField
        label="Name"
        bind:value={name}
        bind:element={nameBox}
        code
        autocomplete="off"
        spellcheck={false}
        placeholder="laptop"
        hint={nameError === undefined ? "Lowercase letters, digits and underscores." : undefined}
        error={nameError}
      />
      <TextField
        label="Description"
        bind:value={description}
        placeholder="Which agent this is for"
      />
    </KeyForm>
  {/if}
</SettingsGroup>

<style>
  .key-created {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .token {
    display: block;
    margin-top: var(--space-8);
    font-size: var(--font-size-xs);
    user-select: all;
  }
</style>
