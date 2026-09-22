<script lang="ts">
  import { onMount } from "svelte";
  import type { AgentKeyInfo } from "../../lib/types";
  import { fetchAgentKeys, storeAgentKey, deleteAgentKey } from "../../lib/api";
  import { toast } from "../../lib/toast.svelte";
  import { userErrorMessage } from "../../lib/errors";
  import ConfirmButton from "../ConfirmButton.svelte";

  const NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;
  const MIN_VALUE_LENGTH = 8;

  let keys = $state<AgentKeyInfo[]>([]);
  let loading = $state(true);
  let loadError = $state("");

  let showAddForm = $state(false);
  let saving = $state(false);
  let newName = $state("");
  let newValue = $state("");
  let newDescription = $state("");

  let trimmedName = $derived(newName.trim());
  let nameValid = $derived(NAME_PATTERN.test(trimmedName));
  let valueValid = $derived(newValue.length >= MIN_VALUE_LENGTH);
  let replacing = $derived(keys.some((k) => k.name === trimmedName));

  onMount(load);

  async function load() {
    loading = true;
    try {
      keys = await fetchAgentKeys();
      loadError = "";
    } catch (err: unknown) {
      loadError = userErrorMessage(err, { action: "Couldn't load agent keys." });
    } finally {
      loading = false;
    }
  }

  function resetForm() {
    newName = "";
    newValue = "";
    newDescription = "";
    showAddForm = false;
  }

  async function handleSave() {
    if (!nameValid || !valueValid || saving) return;
    saving = true;
    try {
      const saved = await storeAgentKey(trimmedName, newValue, newDescription.trim());
      toast.success(`Saved ${saved.name}. Commands that use it get $${saved.env_var}.`);
      resetForm();
      await load();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't save the key." }));
    } finally {
      saving = false;
    }
  }

  async function handleRemove(name: string) {
    try {
      await deleteAgentKey(name);
      toast.success(`Removed ${name}.`);
      await load();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't remove ${name}.` }));
    }
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Agent keys</div>
    <p class="roles-section-hint">
      Credentials the agent can hand to the commands it runs. The agent sees each key's name and
      description, never its value, and values are hidden from everything it reads back.
    </p>

    {#if loading}
      <p class="empty-state">Reading keys.</p>
    {:else if loadError}
      <p class="empty-state agent-key-error">{loadError}</p>
    {:else if keys.length === 0}
      <p class="empty-state">
        No keys yet. Add one here, or ask the agent to save a token it creates.
      </p>
    {/if}

    {#each keys as key (key.name)}
      <div class="mcp-server-entry agent-key-entry" class:minted={key.created_by === "agent"}>
        <div class="mcp-server-info">
          <span class="mcp-server-name">
            {key.name}
            <span class="agent-key-env">${key.env_var}</span>
          </span>
          {#if key.description}
            <span class="agent-key-desc">{key.description}</span>
          {/if}
          {#if key.created_by === "agent"}
            <span class="agent-key-origin">Saved by the agent</span>
          {/if}
        </div>
        <ConfirmButton onConfirm={() => handleRemove(key.name)} title="Remove {key.name}" />
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
          <label for="agent-key-name">Name</label>
          <input
            id="agent-key-name"
            type="text"
            autocomplete="off"
            spellcheck="false"
            bind:value={newName}
            class:input-error={trimmedName !== "" && !nameValid}
            placeholder="github_token"
          />
          <span class="field-hint">
            {#if trimmedName !== "" && !nameValid}
              Start with a lowercase letter; use only lowercase letters, digits, and underscores.
            {:else if nameValid}
              Commands that use it get <code>${trimmedName.toUpperCase()}</code>.{replacing
                ? " Saving replaces the existing key."
                : ""}
            {:else}
              Lowercase letters, digits, and underscores.
            {/if}
          </span>
        </div>
        <div class="settings-field">
          <label for="agent-key-value">Value</label>
          <input
            id="agent-key-value"
            type="password"
            autocomplete="off"
            bind:value={newValue}
            class:input-error={newValue !== "" && !valueValid}
          />
          <span class="field-hint">
            {#if newValue !== "" && !valueValid}
              Use at least {MIN_VALUE_LENGTH} characters so the value can be hidden reliably.
            {:else}
              Stored encrypted. It can't be viewed after saving.
            {/if}
          </span>
        </div>
        <div class="settings-field">
          <label for="agent-key-description">Description</label>
          <input
            id="agent-key-description"
            type="text"
            bind:value={newDescription}
            placeholder="GitHub token with read access to my repos"
          />
          <span class="field-hint">What it's for and what it can reach. The agent reads this.</span>
        </div>
        <div class="mcp-inline-actions">
          <button
            type="submit"
            class="btn btn-primary btn-sm"
            disabled={!nameValid || !valueValid || saving}
          >
            {saving ? "Saving" : "Save key"}
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
        }}>+ Add key</button
      >
    {/if}
  </div>
</div>

<style>
  .agent-key-env {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 400;
    color: var(--text-dim);
    margin-left: 8px;
  }

  .agent-key-desc {
    font-size: 12px;
    color: var(--text-dim);
  }

  /* Keys the agent saved itself carry a moss edge, like lichen grown on stone. */
  .agent-key-entry.minted {
    border-left: 2px solid var(--moss);
  }

  .agent-key-origin {
    font-size: 11px;
    color: var(--moss);
  }

  .agent-key-error {
    color: var(--error);
  }
</style>
