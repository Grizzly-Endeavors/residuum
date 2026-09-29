<script lang="ts">
  import { onMount } from "svelte";
  import { deleteSecret, fetchSecretNames, storeSecret } from "../../lib/api";
  import { toast } from "../../lib/toast.svelte";
  import { userErrorMessage } from "../../lib/errors";
  import Modal from "../Modal.svelte";

  let names = $state<string[]>([]);
  let loading = $state(true);
  let loadError = $state("");

  let showAddForm = $state(false);
  let saving = $state(false);
  let newName = $state("");
  let newValue = $state("");
  let removing = $state<string | null>(null);
  let confirmRemove = $state<string | null>(null);

  let trimmedName = $derived(newName.trim());
  let replacing = $derived(names.includes(trimmedName));

  onMount(load);

  async function load() {
    loading = true;
    try {
      names = await fetchSecretNames();
      loadError = "";
    } catch (err: unknown) {
      loadError = userErrorMessage(err, { action: "Couldn't load secrets." });
    } finally {
      loading = false;
    }
  }

  function resetForm() {
    newName = "";
    newValue = "";
    showAddForm = false;
  }

  async function handleSave() {
    if (trimmedName === "" || newValue === "" || saving) return;
    saving = true;
    try {
      await storeSecret(trimmedName, newValue);
      toast.success(`Saved ${trimmedName}.`);
      resetForm();
      await load();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't save ${trimmedName}.` }));
    } finally {
      saving = false;
    }
  }

  async function handleRemove(name: string) {
    confirmRemove = null;
    removing = name;
    try {
      await deleteSecret(name);
      toast.success(`Removed ${name}.`);
      await load();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't remove ${name}.` }));
    } finally {
      removing = null;
    }
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Secrets</div>
    <p class="roles-section-hint">
      Values that settings refer to by name, such as provider API keys and channel tokens. Every
      agent on this install can use them. Values are stored encrypted and can't be viewed after
      saving.
    </p>

    {#if loading}
      <p class="empty-state">Reading secrets.</p>
    {:else if loadError}
      <p class="empty-state secret-error" role="alert">{loadError}</p>
    {:else if names.length === 0}
      <p class="empty-state">No secrets stored yet.</p>
    {/if}

    {#each names as name (name)}
      <div class="mcp-server-entry">
        <div class="mcp-server-info">
          <span class="mcp-server-name">{name}</span>
        </div>
        <button
          class="btn btn-sm btn-danger"
          onclick={() => {
            confirmRemove = name;
          }}
          disabled={removing === name}
          title="Remove {name}"
          aria-label="Remove {name}"
        >
          {removing === name ? "Removing" : "Remove"}
        </button>
      </div>
    {/each}

    {#if showAddForm}
      <form
        class="mcp-add-form"
        onsubmit={(e) => {
          e.preventDefault();
          void handleSave();
        }}
      >
        <div class="settings-field">
          <label for="secret-name">Name</label>
          <input
            id="secret-name"
            type="text"
            autocomplete="off"
            spellcheck="false"
            bind:value={newName}
            placeholder="openai"
          />
          {#if replacing}
            <span class="field-hint">Saving replaces the existing secret with this name.</span>
          {/if}
        </div>
        <div class="settings-field">
          <label for="secret-value">Value</label>
          <input id="secret-value" type="password" autocomplete="off" bind:value={newValue} />
        </div>
        <div class="mcp-inline-actions">
          <button
            type="submit"
            class="btn btn-primary btn-sm"
            disabled={trimmedName === "" || newValue === "" || saving}
          >
            {saving ? "Saving" : "Save secret"}
          </button>
          <button type="button" class="btn btn-secondary btn-sm" onclick={resetForm}>Cancel</button>
        </div>
      </form>
    {:else}
      <button
        class="btn btn-secondary btn-sm"
        style="margin-top:8px;"
        onclick={() => {
          showAddForm = true;
        }}>+ Add secret</button
      >
    {/if}
  </div>
</div>

<Modal
  open={confirmRemove !== null}
  title="Remove this secret?"
  onClose={() => {
    confirmRemove = null;
  }}
>
  Anything that refers to <strong>{confirmRemove}</strong> stops working until you save it again.
  Its value can't be recovered from here.

  {#snippet actions()}
    <button
      class="btn btn-secondary"
      onclick={() => {
        confirmRemove = null;
      }}>Cancel</button
    >
    <button
      class="btn btn-danger"
      onclick={() => {
        if (confirmRemove !== null) void handleRemove(confirmRemove);
      }}>Remove</button
    >
  {/snippet}
</Modal>

<style>
  .secret-error {
    color: var(--error);
  }
</style>
