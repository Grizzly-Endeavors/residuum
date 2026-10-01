<script lang="ts">
  import { onMount } from "svelte";
  import type { ConfigFields } from "../../lib/settings-toml";
  import type { A2aKeyInfo } from "../../lib/types";
  import { fetchA2aKeys, createA2aKey, revokeA2aKey } from "../../lib/api";
  import { toast } from "../../lib/toast.svelte";
  import { userErrorMessage } from "../../lib/errors";
  import { Icon } from "../../lib/icons";
  import { notifyWithUndo } from "../../lib/undo";

  const NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;

  // The install's listener and the caller keys, which every agent shares.
  let { fields = $bindable() }: { fields: ConfigFields } = $props();

  // ── Caller keys ───────────────────────────────────────────────────────

  let keys = $state<A2aKeyInfo[]>([]);
  let keysLoading = $state(true);
  let keysError = $state("");

  let showAddKeyForm = $state(false);
  let newKeyName = $state("");
  let newKeyDescription = $state("");
  let creatingKey = $state(false);

  let mintedToken = $state<{ name: string; token: string } | null>(null);
  let tokenCopied = $state(false);
  let tokenCopyTimer: ReturnType<typeof setTimeout> | undefined;

  let trimmedKeyName = $derived(newKeyName.trim());
  let keyNameValid = $derived(NAME_PATTERN.test(trimmedKeyName));
  let keyReplacing = $derived(keys.some((k) => k.name === trimmedKeyName));

  async function loadKeys() {
    keysLoading = true;
    try {
      keys = await fetchA2aKeys();
      keysError = "";
    } catch (err: unknown) {
      keysError = userErrorMessage(err, { action: "Couldn't load caller keys." });
    } finally {
      keysLoading = false;
    }
  }

  function resetKeyForm() {
    newKeyName = "";
    newKeyDescription = "";
    showAddKeyForm = false;
  }

  async function handleCreateKey() {
    if (!keyNameValid || creatingKey) return;
    creatingKey = true;
    try {
      const created = await createA2aKey(trimmedKeyName, newKeyDescription.trim());
      mintedToken = { name: created.name, token: created.token };
      resetKeyForm();
      await loadKeys();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't create the key." }));
    } finally {
      creatingKey = false;
    }
  }

  async function copyMintedToken() {
    if (!mintedToken) return;
    try {
      await navigator.clipboard.writeText(mintedToken.token);
      tokenCopied = true;
      clearTimeout(tokenCopyTimer);
      tokenCopyTimer = setTimeout(() => {
        tokenCopied = false;
      }, 1800);
    } catch {
      // Clipboard write can fail in insecure contexts; the token is already
      // visible on screen, so this is non-blocking.
    }
  }

  async function handleRevokeKey(name: string) {
    try {
      const checkpointId = await revokeA2aKey(name);
      notifyWithUndo(
        null,
        `Revoked ${name}. It can no longer reach this agent.`,
        "hub",
        "a2a-keys.toml",
        checkpointId,
        loadKeys,
      );
      await loadKeys();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't revoke ${name}.` }));
    }
  }

  onMount(() => {
    void loadKeys();
  });
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Listener</div>

    <p class="field-hint">
      One listener serves every agent on this install. Each agent's own visibility is set in its
      settings.
    </p>

    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.a2a_enabled} />
          <span class="toggle-slider"></span>
        </span>
        Let other agents reach this install
      </label>
      <span class="field-hint">
        Off: nothing outside this install can reach its agents over A2A. On: other agents can find
        them, and anyone with a caller key can hand them work.
      </span>
    </div>

    {#if fields.a2a_enabled}
      <div class="settings-field">
        <label for="a2a-port">Listener port</label>
        <input
          id="a2a-port"
          type="number"
          bind:value={fields.a2a_port}
          placeholder="Default: 7702"
        />
        <span class="field-hint"
          >Expose only this port through your own tunnel, if you run one.</span
        >
      </div>
      <div class="settings-field">
        <label for="a2a-public-url">Your own address</label>
        <input
          id="a2a-public-url"
          type="text"
          bind:value={fields.a2a_public_url}
          placeholder="https://your-own-tunnel.example/a2a (optional)"
        />
        <span class="field-hint"
          >The Residuum relay gives each agent its own address, so this is only needed if you run
          your own tunnel. Each agent is served under <code>/agents/name</code> at this address.</span
        >
      </div>
    {/if}
  </div>

  <!-- Caller keys -->
  <div class="settings-group">
    <div class="settings-group-label">Caller keys</div>
    <p class="roles-section-hint">
      Tokens other agents present to reach this one. Give a key to an agent you want to let in;
      revoking one removes that agent's access right away.
    </p>

    {#if mintedToken}
      <div class="a2a-token-reveal emerges" role="status" aria-live="polite">
        <div class="a2a-token-eyebrow">Key for {mintedToken.name} created</div>
        <div class="a2a-token-row">
          <code class="a2a-token-value">{mintedToken.token}</code>
          <button
            class="copy-btn"
            onclick={copyMintedToken}
            title="Copy token"
            aria-label="Copy token"
          >
            {#if tokenCopied}
              <Icon name="check" size={14} />
              <span>Copied</span>
            {:else}
              <Icon name="copy" size={14} />
              <span>Copy</span>
            {/if}
          </button>
        </div>
        <p class="a2a-token-hint">
          Copy it now — you won't see this again. Give it to the agent as an
          <code>Authorization: Bearer</code> header.
        </p>
        <button
          class="btn btn-sm btn-secondary"
          onclick={() => {
            mintedToken = null;
          }}>Done</button
        >
      </div>
    {/if}

    {#if keysLoading}
      <p class="empty-state">Reading caller keys.</p>
    {:else if keysError}
      <p class="empty-state a2a-error">{keysError}</p>
    {:else if keys.length === 0}
      <p class="empty-state">No caller keys yet. Add one for each agent you want to let in.</p>
    {/if}

    {#each keys as key (key.name)}
      <div class="mcp-server-entry">
        <div class="mcp-server-info">
          <span class="mcp-server-name">{key.name}</span>
          {#if key.description}
            <span class="agent-key-desc">{key.description}</span>
          {/if}
        </div>
        <button
          class="btn btn-sm btn-danger"
          onclick={() => handleRevokeKey(key.name)}
          title="Revoke {key.name}"
        >
          Revoke
        </button>
      </div>
    {/each}

    {#if showAddKeyForm}
      <form
        class="mcp-add-form"
        onsubmit={(e) => {
          e.preventDefault();
          void handleCreateKey();
        }}
      >
        <div class="settings-field">
          <label for="a2a-key-name">Name</label>
          <input
            id="a2a-key-name"
            type="text"
            autocomplete="off"
            spellcheck="false"
            bind:value={newKeyName}
            class:input-error={trimmedKeyName !== "" && !keyNameValid}
            placeholder="laptop"
          />
          <span class="field-hint">
            {#if trimmedKeyName !== "" && !keyNameValid}
              Start with a lowercase letter; use only lowercase letters, digits, and underscores.
            {:else if keyReplacing}
              A key named {trimmedKeyName} already exists.
            {:else}
              Lowercase letters, digits, and underscores.
            {/if}
          </span>
        </div>
        <div class="settings-field">
          <label for="a2a-key-description">Description</label>
          <input
            id="a2a-key-description"
            type="text"
            bind:value={newKeyDescription}
            placeholder="What agent this is for"
          />
        </div>
        <div class="mcp-inline-actions">
          <button
            type="submit"
            class="btn btn-primary btn-sm"
            disabled={!keyNameValid || keyReplacing || creatingKey}
          >
            {creatingKey ? "Creating" : "Create key"}
          </button>
          <button type="button" class="btn btn-secondary btn-sm" onclick={resetKeyForm}
            >Cancel</button
          >
        </div>
      </form>
    {:else}
      <button
        class="btn btn-secondary btn-sm"
        style="margin-top:8px;"
        onclick={() => {
          showAddKeyForm = true;
        }}>+ Add key</button
      >
    {/if}
  </div>
</div>

<style>
  .a2a-error {
    color: var(--error);
  }

  /* Hints that sit directly in a section rather than inside a
     .settings-field, which is the only place the shared rule styles them. */
  p.field-hint {
    font-size: 12px;
    color: var(--text-dim);
    margin: 4px 0 8px;
  }

  .copy-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--text-dim);
    font-size: 12px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .copy-btn:hover {
    color: var(--text);
    background: var(--bg-deep);
    border-color: var(--border);
  }

  .agent-key-desc {
    font-size: 12px;
    color: var(--text-dim);
    display: block;
  }

  .a2a-token-reveal {
    border: 1px solid var(--moss);
    border-radius: 6px;
    padding: 12px;
    margin-bottom: 12px;
    background: var(--bg-deep);
  }

  .a2a-token-eyebrow {
    font-size: 12px;
    color: var(--moss);
    margin-bottom: 6px;
  }

  .a2a-token-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .a2a-token-value {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--text);
    word-break: break-all;
    user-select: all;
  }

  .a2a-token-hint {
    font-size: 12px;
    color: var(--text-dim);
    margin: 8px 0;
  }
</style>
