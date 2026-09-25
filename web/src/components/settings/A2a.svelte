<script lang="ts">
  import { onMount } from "svelte";
  import type { ConfigFields } from "../../lib/settings-toml";
  import type {
    A2aStatusResponse,
    A2aKeyInfo,
    A2aAgentCard,
    A2aRemoteAgent,
  } from "../../lib/types";
  import {
    fetchA2aStatus,
    fetchA2aCard,
    fetchA2aKeys,
    createA2aKey,
    revokeA2aKey,
    fetchA2aAgents,
    fetchA2aAgentsRaw,
    putA2aAgentsRaw,
  } from "../../lib/api";
  import { toast } from "../../lib/toast.svelte";
  import { userErrorMessage } from "../../lib/errors";
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { notifyWithUndo } from "../../lib/undo";

  const NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;

  let { fields = $bindable(), simple = false }: { fields: ConfigFields; simple?: boolean } =
    $props();

  // ── Status ────────────────────────────────────────────────────────────

  let status = $state<A2aStatusResponse | null>(null);
  let statusLoading = $state(true);
  let statusError = $state("");
  let urlCopied = $state(false);
  let urlCopyTimer: ReturnType<typeof setTimeout> | undefined;

  async function loadStatus() {
    statusLoading = true;
    try {
      status = await fetchA2aStatus();
      statusError = "";
    } catch (err: unknown) {
      statusError = userErrorMessage(err, { action: "Couldn't check the A2A connection." });
    } finally {
      statusLoading = false;
    }
  }

  async function copyPublicUrl() {
    if (!status?.public_url) return;
    try {
      await navigator.clipboard.writeText(status.public_url);
      urlCopied = true;
      clearTimeout(urlCopyTimer);
      urlCopyTimer = setTimeout(() => {
        urlCopied = false;
      }, 1800);
    } catch {
      // Clipboard write can fail in insecure contexts; the address is
      // already visible on screen, so this is non-blocking.
    }
  }

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
      await revokeA2aKey(name);
      notifyWithUndo(
        `Revoked ${name}. It can no longer reach this agent.`,
        "config",
        "a2a-keys.toml",
        loadKeys,
      );
      await loadKeys();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't revoke ${name}.` }));
    }
  }

  // ── Remote agents ─────────────────────────────────────────────────────

  let agents = $state<A2aRemoteAgent[]>([]);
  let agentsLoading = $state(true);
  let agentsError = $state("");

  let rawMode = $state(false);
  let rawAgents = $state("");
  let rawAgentsEdit = $state("");
  let rawAgentsSaving = $state(false);
  let rawAgentsError = $state("");

  async function loadAgents() {
    agentsLoading = true;
    try {
      agents = await fetchA2aAgents();
      agentsError = "";
    } catch (err: unknown) {
      agentsError = userErrorMessage(err, {
        action: "Couldn't load remote agents.",
        notFound: "Remote agents aren't available in this build yet.",
      });
    } finally {
      agentsLoading = false;
    }
  }

  async function enterRawMode() {
    try {
      rawAgents = await fetchA2aAgentsRaw();
      rawAgentsEdit = rawAgents;
      rawAgentsError = "";
      rawMode = true;
    } catch (err: unknown) {
      toast.error(
        userErrorMessage(err, {
          action: "Couldn't load a2a.json.",
          notFound: "Remote agents aren't available in this build yet.",
        }),
      );
    }
  }

  function cancelRawMode() {
    rawMode = false;
    rawAgentsEdit = rawAgents;
    rawAgentsError = "";
  }

  /**
   * `a2a.json` always saves now, even when invalid — the loader skips an
   * unusable agent entry with a warning and keeps every other agent
   * running, so an invalid save is reported with a diagnostic rather than
   * left unsaved. Raw mode stays open when there's a problem to fix;
   * otherwise it closes like a normal successful save.
   */
  async function saveRawAgents() {
    if (rawAgentsSaving) return;
    rawAgentsSaving = true;
    rawAgentsError = "";
    try {
      const result = await putA2aAgentsRaw(rawAgentsEdit);
      rawAgents = rawAgentsEdit;
      await loadAgents();
      if (!result.valid) {
        rawAgentsError = result.error ?? "a2a.json saved, but has a problem.";
        return;
      }
      rawMode = false;
      toast.success("Saved a2a.json.");
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't save a2a.json." }));
    } finally {
      rawAgentsSaving = false;
    }
  }

  function statusLabel(agent: A2aRemoteAgent): string {
    switch (agent.status) {
      case "ok":
        return "Reachable";
      case "error":
        return "Unreachable";
      case "pending":
        return "Checking";
    }
  }

  // ── Agent card preview ──────────────────────────────────────────────

  let card = $state<A2aAgentCard | null>(null);
  let cardLoading = $state(true);
  let cardError = $state("");

  async function loadCard() {
    cardLoading = true;
    try {
      card = await fetchA2aCard();
      cardError = "";
    } catch (err: unknown) {
      cardError = userErrorMessage(err, { action: "Couldn't load the agent card." });
    } finally {
      cardLoading = false;
    }
  }

  function openWorkspace() {
    router.setWorkspace(true);
  }

  onMount(() => {
    void loadStatus();
    void loadKeys();
    void loadAgents();
    void loadCard();
  });
</script>

<div class="settings-section">
  <!-- Status -->
  <div class="settings-group">
    <div class="settings-group-label">Status</div>

    {#if statusLoading}
      <p class="empty-state">Checking the A2A connection.</p>
    {:else if statusError}
      <p class="empty-state a2a-error">{statusError}</p>
    {:else if status}
      <div class="a2a-status-row">
        <span class="a2a-status-dot" class:on={status.enabled} class:off={!status.enabled}></span>
        <span class="a2a-status-text">{status.enabled ? "On" : "Off"}</span>
        <span class="a2a-visibility-badge">{status.visibility}</span>
      </div>
      <p class="field-hint">
        {#if status.visibility === "public"}
          Public: any agent that finds the address below can see what this one can do. Reaching it
          beyond that still needs a caller key.
        {:else}
          Private: hidden from anyone without a caller key for this agent — even seeing what it can
          do requires one.
        {/if}
      </p>

      {#if status.enabled}
        {#if status.public_url}
          <div class="a2a-address-row">
            <code class="a2a-address">{status.public_url}</code>
            <button
              class="copy-btn"
              onclick={copyPublicUrl}
              title="Copy address"
              aria-label="Copy address"
            >
              {#if urlCopied}
                <Icon name="check" size={14} />
                <span>Copied</span>
              {:else}
                <Icon name="copy" size={14} />
                <span>Copy</span>
              {/if}
            </button>
          </div>
        {:else}
          <p class="field-hint">
            No address yet. Connect to the relay, or set an address of your own below, so other
            agents have a way to reach this one.
          </p>
        {/if}

        {#if !status.listener_running}
          <p class="validation-msg error">
            Nothing is answering on port {status.port} right now. Restart the agent, or check its logs,
            to find out why.
          </p>
        {/if}
      {/if}

      {#if status.card_error}
        <p class="validation-msg error">{status.card_error}</p>
      {/if}
    {/if}

    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.a2a_enabled} />
          <span class="toggle-slider"></span>
        </span>
        Let other agents reach this one
      </label>
      <span class="field-hint">
        Off: nothing outside this agent can reach it over A2A. On: other agents can find it, and
        anyone with a caller key can hand it work.
      </span>
    </div>

    {#if fields.a2a_enabled}
      <div class="settings-field">
        <label for="a2a-visibility">Who can see it</label>
        <select id="a2a-visibility" bind:value={fields.a2a_visibility}>
          <option value="">Public (default)</option>
          <option value="private">Private</option>
        </select>
      </div>
    {/if}

    {#if !simple && fields.a2a_enabled}
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
        <span class="field-hint">Only needed if you run your own tunnel instead of the relay.</span>
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

  <!-- Remote agents -->
  <div class="settings-group">
    <div class="settings-group-label">Remote agents</div>
    <p class="roles-section-hint">
      Agents this one can hand work to. Listed here from <code>config/a2a.json</code>, plus any of
      your other agents found automatically once connected to the relay.
    </p>

    {#if rawMode}
      <textarea class="toml-editor" bind:value={rawAgentsEdit}></textarea>
      {#if rawAgentsError}
        <p class="validation-msg error">{rawAgentsError}</p>
      {/if}
      <div class="mcp-inline-actions">
        <button class="btn btn-primary btn-sm" onclick={saveRawAgents} disabled={rawAgentsSaving}>
          {rawAgentsSaving ? "Saving" : "Save"}
        </button>
        <button class="btn btn-secondary btn-sm" onclick={cancelRawMode} disabled={rawAgentsSaving}
          >Cancel</button
        >
      </div>
    {:else}
      {#if agentsLoading}
        <p class="empty-state">Reading remote agents.</p>
      {:else if agentsError}
        <p class="empty-state a2a-error">{agentsError}</p>
      {:else if agents.length === 0}
        <p class="empty-state">
          None configured yet. Add one in the raw editor below, or connect to the relay to find your
          other agents automatically.
        </p>
      {/if}

      {#each agents as agent (agent.name)}
        <div class="mcp-server-entry a2a-remote-agent">
          <div class="mcp-server-info">
            <span class="mcp-server-name">
              {agent.name}
              <span class="a2a-source-badge">{agent.source}</span>
            </span>
            <span class="mcp-server-cmd">{agent.url}</span>
            <div class="a2a-status-row">
              <span
                class="a2a-status-dot"
                class:on={agent.status === "ok"}
                class:off={agent.status === "error"}
                class:pending={agent.status === "pending"}
              ></span>
              <span class="a2a-status-text">{statusLabel(agent)}</span>
            </div>
            {#if agent.status === "error" && agent.error}
              <span class="agent-key-desc a2a-error">{agent.error}</span>
            {/if}
            {#if agent.card}
              <span class="agent-key-desc">{agent.card.description}</span>
              {#if agent.card.skills.length > 0}
                <div class="a2a-skill-chips">
                  {#each agent.card.skills as skill (skill.id)}
                    <span class="a2a-skill-chip">{skill.name}</span>
                  {/each}
                </div>
              {/if}
            {/if}
          </div>
        </div>
      {/each}

      <button
        class="btn btn-secondary btn-sm"
        style="margin-top:8px;"
        onclick={() => {
          void enterRawMode();
        }}>Edit a2a.json</button
      >
    {/if}
  </div>

  <!-- Agent card preview -->
  <div class="settings-group">
    <div class="settings-group-label">Agent card</div>
    <p class="roles-section-hint">
      What this agent shows to other agents that reach it. Edited in the workspace file
      <code>config/agent-card.json</code>.
    </p>

    {#if cardLoading}
      <p class="empty-state">Reading the agent card.</p>
    {:else if cardError}
      <p class="empty-state a2a-error">{cardError}</p>
    {:else if card}
      <div class="a2a-card-preview">
        <div class="a2a-card-name">{card.name}</div>
        <p class="a2a-card-desc">{card.description}</p>
        {#if card.skills.length > 0}
          <div class="a2a-skill-chips">
            {#each card.skills as skill (skill.id)}
              <span class="a2a-skill-chip" title={skill.description}>{skill.name}</span>
            {/each}
          </div>
        {:else}
          <p class="field-hint">No skills listed yet.</p>
        {/if}
      </div>
    {/if}

    <button class="btn btn-secondary btn-sm" style="margin-top:8px;" onclick={openWorkspace}>
      Open workspace
    </button>
  </div>
</div>

<style>
  .a2a-error {
    color: var(--error);
  }

  .a2a-status-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 8px 0;
  }

  .a2a-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--text-dim);
  }

  .a2a-status-dot.on {
    background: var(--moss);
    box-shadow: 0 0 6px rgba(107, 122, 74, 0.5);
  }

  .a2a-status-dot.off {
    background: var(--text-dim);
  }

  .a2a-status-dot.pending {
    background: var(--vein-bright);
    animation: a2a-pulse-dot 1.5s infinite;
  }

  @keyframes a2a-pulse-dot {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }

  .a2a-status-text {
    font-size: 12px;
    font-weight: 500;
  }

  /* Hints that sit directly in a section rather than inside a
     .settings-field, which is the only place the shared rule styles them. */
  p.field-hint {
    font-size: 12px;
    color: var(--text-dim);
    margin: 4px 0 8px;
  }

  .a2a-visibility-badge {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 1px 6px;
    text-transform: lowercase;
  }

  .a2a-address-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 8px 0;
  }

  .a2a-address {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--vein-bright);
    overflow-wrap: anywhere;
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

  .a2a-source-badge {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
    margin-left: 8px;
  }

  .a2a-remote-agent {
    align-items: flex-start;
  }

  .a2a-skill-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }

  .a2a-skill-chip {
    font-size: 11px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1px 8px;
  }

  .a2a-card-preview {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 12px;
    background: var(--bg-deep);
  }

  .a2a-card-name {
    font-weight: 500;
  }

  .a2a-card-desc {
    font-size: 13px;
    color: var(--text-dim);
    margin: 4px 0 8px;
  }
</style>
