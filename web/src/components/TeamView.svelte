<script lang="ts">
  import { onMount } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { legacyRouter } from "../lib/legacy-router.svelte";
  import { agentNameProblem } from "../lib/agent-name";
  import { stateLabel, unreadText } from "../lib/agent-state";
  import { relativeTime } from "../lib/time";
  import type { A2aVisibility, AgentSummary } from "../lib/hub-types";
  import AgentStateGlyph from "./AgentStateGlyph.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  type Action = "start" | "stop" | "restart" | "autostart" | "visibility" | "delete" | "restore";

  // What each agent is waiting on right now, so its buttons show progress.
  let pending = $state<Record<string, Action | undefined>>({});

  const PENDING_LABELS: Record<"start" | "stop" | "restart", string> = {
    start: "Starting",
    stop: "Stopping",
    restart: "Restarting",
  };

  const LIFECYCLE = ["start", "stop", "restart"] as const;
  const LABELS: Record<(typeof LIFECYCLE)[number], string> = {
    start: "Start",
    stop: "Stop",
    restart: "Restart",
  };

  function lifecycle(action: (typeof LIFECYCLE)[number], name: string): Promise<unknown> {
    if (action === "start") return hub.startAgent(name);
    if (action === "stop") return hub.stopAgent(name);
    return hub.restartAgent(name);
  }

  async function run(name: string, action: Action, call: () => Promise<unknown>): Promise<void> {
    if (pending[name] !== undefined) return;
    pending[name] = action;
    try {
      await call();
    } finally {
      pending[name] = undefined;
    }
  }

  function canStart(agent: AgentSummary): boolean {
    return agent.state === "stopped" || agent.state === "failed";
  }
  function canStop(agent: AgentSummary): boolean {
    return agent.state === "running" || agent.state === "starting";
  }
  function canRestart(agent: AgentSummary): boolean {
    return agent.state === "running" || agent.state === "failed";
  }

  /** Why a lifecycle button is unavailable, or undefined when it is available. */
  function unavailableReason(
    agent: AgentSummary,
    action: "start" | "stop" | "restart",
  ): string | undefined {
    if (action === "start" && !canStart(agent)) {
      return agent.state === "starting"
        ? `${agent.name} is starting`
        : `${agent.name} is already running`;
    }
    if (action === "stop" && !canStop(agent)) return `${agent.name} is not running`;
    if (action === "restart" && !canRestart(agent)) {
      return agent.state === "starting"
        ? `${agent.name} is still starting`
        : `${agent.name} is not running`;
    }
    return undefined;
  }

  async function toggleAutostart(agent: AgentSummary, input: HTMLInputElement): Promise<void> {
    const wanted = input.checked;
    await run(agent.name, "autostart", async () => {
      const saved = await hub.setAutostart(agent.name, wanted);
      // The list is the source of truth: put the box back if the change didn't land.
      if (!saved) input.checked = agent.autostart;
    });
  }

  // Applied at once so the select never lags the choice; put back if it doesn't land.
  async function changeVisibility(agent: AgentSummary, select: HTMLSelectElement): Promise<void> {
    const wanted = select.value as A2aVisibility;
    await run(agent.name, "visibility", async () => {
      const saved = await hub.setVisibility(agent.name, wanted);
      if (!saved) select.value = agent.a2a_visibility;
    });
  }

  onMount(() => {
    void hub.refreshDeleted();
  });

  // ── Delete and restore ──────────────────────────────────────────────

  let confirmDelete = $state<string | null>(null);
  /** Deletions from this visit, with the checkpoint that holds each one's files. */
  let deleted = $state<{ name: string; checkpointId: string | null }[]>([]);

  /** Deleted agents not already shown in a note above with its own Undo. */
  let recent = $derived(hub.deleted.filter((d) => !deleted.some((note) => note.name === d.name)));

  /** Restore a deleted agent; `checkpointId` is the one its deletion took, else the hub picks. */
  async function restore(name: string, checkpointId: string | null): Promise<void> {
    await run(name, "restore", async () => {
      const agent = await hub.restoreAgent(name, checkpointId ?? undefined);
      if (agent) deleted = deleted.filter((d) => d.name !== name);
    });
  }

  async function deleteConfirmed(): Promise<void> {
    const name = confirmDelete;
    if (name === null) return;
    confirmDelete = null;
    await run(name, "delete", async () => {
      const result = await hub.deleteAgent(name);
      if (result) {
        deleted = [{ name, checkpointId: result.checkpoint_id }, ...deleted];
      }
    });
  }

  // ── Create ──────────────────────────────────────────────────────────

  let newName = $state("");
  let newDescription = $state("");
  let modelsFrom = $state("");
  let visibility = $state<A2aVisibility>("private");
  let creating = $state(false);
  let created = $state<string | null>(null);
  let nameTouched = $state(false);

  let nameProblem = $derived.by(() => {
    const problem = agentNameProblem(newName);
    if (problem !== null) return problem;
    if (hub.agent(newName)) return `An agent named "${newName}" already exists.`;
    return null;
  });
  // Show a problem once the user has typed something, not on an empty form.
  let shownProblem = $derived(nameTouched && newName !== "" ? nameProblem : null);

  // The model settings have to come from somewhere: default to the first
  // agent, and fall back when the chosen one is deleted.
  $effect(() => {
    if (hub.agents.length === 0) {
      modelsFrom = "";
    } else if (!hub.agents.some((a) => a.name === modelsFrom)) {
      modelsFrom = hub.agents[0]?.name ?? "";
    }
  });

  let canCreate = $derived(nameProblem === null && modelsFrom !== "" && !creating);

  async function create(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    nameTouched = true;
    if (!canCreate) return;
    creating = true;
    created = null;
    try {
      const description = newDescription.trim();
      const agent = await hub.createAgent({
        name: newName,
        description: description !== "" ? description : null,
        models_from: modelsFrom,
        providers_toml: null,
        a2a_visibility: visibility,
      });
      if (agent) {
        created = agent.name;
        newName = "";
        newDescription = "";
        visibility = "private";
        nameTouched = false;
      }
    } finally {
      creating = false;
    }
  }
</script>

<section class="team-view emerges" aria-labelledby="team-title">
  <header class="team-head">
    <div>
      <h2 id="team-title" class="team-title">Team</h2>
      <p class="team-sub">
        Every agent on this install. Start, stop or restart them here, or add a new one.
      </p>
    </div>
    <button type="button" class="btn btn-secondary btn-sm" onclick={onClose}>Close</button>
  </header>

  <p id="team-visibility-hint" class="team-visibility-hint">
    <strong>A2A card.</strong> Public shows only an agent's card to other agents. Everything else, including
    handing it work, still needs a caller key.
  </p>

  {#if deleted.length > 0}
    <ul class="team-deleted" role="status">
      {#each deleted as note (note.name)}
        <li>
          {#if note.checkpointId}
            <strong>{note.name}</strong> was deleted. Its directory is gone, and checkpoint
            <code>{note.checkpointId}</code> holds its files so it can be restored.
          {:else}
            <strong>{note.name}</strong> was deleted. No checkpoint was taken, so its files can't be restored.
          {/if}
          <span class="team-deleted-actions">
            {#if note.checkpointId}
              <button
                type="button"
                class="btn btn-primary btn-sm"
                class:is-pending={pending[note.name] === "restore"}
                disabled={pending[note.name] !== undefined}
                aria-label="Undo deleting {note.name}"
                onclick={() => void restore(note.name, note.checkpointId)}
                >{pending[note.name] === "restore" ? "Restoring" : "Undo"}</button
              >
            {/if}
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              aria-label="Dismiss note about {note.name}"
              onclick={() => {
                deleted = deleted.filter((d) => d.name !== note.name);
              }}>Dismiss</button
            >
          </span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if hub.agents.length === 0}
    <p class="team-empty">No agents yet. Create one below.</p>
  {:else}
    <ul class="team-list">
      {#each hub.agents as agent (agent.name)}
        {@const activity = hub.activityOf(agent.name)}
        {@const busyAction = pending[agent.name]}
        <li class="team-row state-{agent.state}" aria-busy={busyAction !== undefined}>
          <div class="team-row-main">
            <div class="team-row-title">
              <AgentStateGlyph state={agent.state} />
              <button
                type="button"
                class="team-agent-link"
                onclick={() => {
                  legacyRouter.openAgent(agent.name);
                }}>{agent.name}</button
              >
              <span class="team-state">{stateLabel(agent.state)}</span>
              {#if activity.busy}
                <span class="team-chip">working</span>
              {/if}
              {#if activity.unread > 0}
                <span class="team-chip team-chip-unread">{unreadText(activity.unread)} unread</span>
              {/if}
            </div>
            <p class="team-role">{agent.role ?? "No role page yet."}</p>
            {#if agent.state === "failed" && agent.last_error}
              <p class="team-error">
                {agent.last_error.message}
                <span class="team-error-at">{relativeTime(agent.last_error.at)}</span>
              </p>
            {/if}
          </div>

          <div class="team-row-controls">
            <div class="team-row-settings">
              <label class="team-visibility-select">
                <span>A2A card</span>
                <select
                  class="select"
                  value={agent.a2a_visibility}
                  disabled={busyAction !== undefined}
                  aria-label="A2A card for {agent.name}"
                  aria-describedby="team-visibility-hint"
                  onchange={(e) => void changeVisibility(agent, e.currentTarget)}
                >
                  <option value="private">Private</option>
                  <option value="public">Public</option>
                </select>
              </label>
              <label class="team-autostart">
                <span class="toggle-switch">
                  <input
                    type="checkbox"
                    checked={agent.autostart}
                    disabled={busyAction !== undefined}
                    aria-label="Start automatically for {agent.name}"
                    onchange={(e) => void toggleAutostart(agent, e.currentTarget)}
                  />
                  <span class="toggle-slider"></span>
                </span>
                <span>Start automatically</span>
              </label>
            </div>
            <div class="team-buttons" role="group" aria-label="{agent.name} lifecycle">
              {#each LIFECYCLE as action (action)}
                {@const reason = unavailableReason(agent, action)}
                <button
                  type="button"
                  class="btn btn-secondary btn-sm"
                  class:is-pending={busyAction === action}
                  disabled={reason !== undefined || busyAction !== undefined}
                  title={busyAction === undefined ? reason : undefined}
                  aria-label="{busyAction === action
                    ? PENDING_LABELS[action]
                    : LABELS[action]} {agent.name}"
                  onclick={() => void run(agent.name, action, () => lifecycle(action, agent.name))}
                >
                  {busyAction === action ? PENDING_LABELS[action] : LABELS[action]}
                </button>
              {/each}
              <button
                type="button"
                class="btn btn-danger btn-sm"
                class:is-pending={busyAction === "delete"}
                disabled={busyAction !== undefined}
                aria-label="Delete {agent.name}"
                onclick={() => {
                  confirmDelete = agent.name;
                }}
              >
                {busyAction === "delete" ? "Deleting" : "Delete"}
              </button>
            </div>
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  {#if hub.deletedError !== null}
    <section class="team-recent" aria-labelledby="recent-title">
      <h3 id="recent-title" class="team-recent-title">Recently deleted</h3>
      <p class="team-error" role="alert">
        {hub.deletedError}
        <button
          type="button"
          class="btn btn-secondary btn-sm"
          onclick={() => void hub.refreshDeleted()}>Try again</button
        >
      </p>
    </section>
  {:else if recent.length > 0}
    <section class="team-recent" aria-labelledby="recent-title">
      <h3 id="recent-title" class="team-recent-title">Recently deleted</h3>
      <p class="team-recent-hint">
        A deleted agent's files stay in the checkpoint history. Restoring brings back its notes,
        memory, settings and role page.
      </p>
      <ul class="team-recent-list">
        {#each recent as gone (gone.name)}
          {@const busyAction = pending[gone.name]}
          <li class="team-recent-row" aria-busy={busyAction !== undefined}>
            <div class="team-recent-main">
              <span class="team-recent-name">{gone.name}</span>
              <span class="team-recent-when">deleted {relativeTime(gone.deleted_at)}</span>
            </div>
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              class:is-pending={busyAction === "restore"}
              disabled={busyAction !== undefined}
              aria-label="Restore {gone.name}"
              onclick={() => void restore(gone.name, gone.checkpoint_id)}
              >{busyAction === "restore" ? "Restoring" : "Restore"}</button
            >
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <form class="team-create" onsubmit={create} novalidate aria-labelledby="create-title">
    <h3 id="create-title" class="team-create-title">Create an agent</h3>

    <div class="settings-field">
      <label for="create-name">Name</label>
      <input
        id="create-name"
        type="text"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        bind:value={newName}
        onblur={() => {
          nameTouched = true;
        }}
        class:input-error={shownProblem !== null}
        aria-invalid={shownProblem !== null}
        aria-describedby="create-name-hint"
        placeholder="research-buddy"
      />
      <span
        id="create-name-hint"
        class="field-hint"
        class:team-hint-error={shownProblem !== null}
        role={shownProblem !== null ? "alert" : undefined}
      >
        {shownProblem ??
          "Up to 24 lowercase letters, digits and hyphens. It names the agent's directory."}
      </span>
    </div>

    <div class="settings-field">
      <label for="create-description">Description (optional)</label>
      <textarea
        id="create-description"
        rows="3"
        bind:value={newDescription}
        placeholder="Keeps my reading list and summarizes new papers each morning."
      ></textarea>
      <span class="field-hint">
        Say what this agent is for. It turns the description into its own notes (its role page and
        SOUL.md) and fills in the rest on its first turn.
      </span>
    </div>

    <div class="settings-field">
      <label for="create-models">Copy model settings from</label>
      <select id="create-models" bind:value={modelsFrom} disabled={hub.agents.length === 0}>
        {#each hub.agents as agent (agent.name)}
          <option value={agent.name}>{agent.name}</option>
        {/each}
      </select>
      <span class="field-hint">
        {#if hub.agents.length === 0}
          There is no agent to copy from yet.
        {:else}
          The new agent starts with this agent's providers and model choices.
        {/if}
      </span>
    </div>

    <fieldset class="team-visibility">
      <legend>A2A visibility</legend>
      <label>
        <input type="radio" name="create-visibility" value="private" bind:group={visibility} />
        Private
        <span class="option-hint">Other agents need a caller key to even see it.</span>
      </label>
      <label>
        <input type="radio" name="create-visibility" value="public" bind:group={visibility} />
        Public
        <span class="option-hint">Anyone who finds the address can see what it can do.</span>
      </label>
    </fieldset>

    <div class="team-create-actions">
      <button type="submit" class="btn btn-primary" disabled={!canCreate} aria-busy={creating}>
        {creating ? "Creating" : "Create agent"}
      </button>
      <span class="team-created" role="status">
        {#if created}
          Created {created}.
          <button
            type="button"
            class="team-agent-link"
            onclick={() => {
              if (created) legacyRouter.openAgent(created);
            }}>Open it</button
          >
        {/if}
      </span>
    </div>
  </form>
</section>

<Modal
  open={confirmDelete !== null}
  title="Delete {confirmDelete ?? ''}?"
  onClose={() => {
    confirmDelete = null;
  }}
>
  This removes <strong>{confirmDelete}</strong>'s directory: its notes, memory and settings. A
  checkpoint is taken first, so you can undo this and restore it afterwards.

  {#snippet actions()}
    <button
      type="button"
      class="btn btn-secondary"
      onclick={() => {
        confirmDelete = null;
      }}>Cancel</button
    >
    <button type="button" class="btn btn-danger" onclick={() => void deleteConfirmed()}
      >Delete agent</button
    >
  {/snippet}
</Modal>

<style>
  .team-view {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    /* Bottom room so the fixed notification corner never sits on the last controls. */
    padding: var(--s-5) var(--s-4) var(--s-8);
    width: 100%;
    max-width: 960px;
    margin: 0 auto;
  }

  .team-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--s-3);
    margin-bottom: var(--s-5);
  }

  .team-title {
    font-family: var(--font-display);
    font-size: var(--fs-xl);
    font-weight: 500;
    letter-spacing: 0.1em;
    margin: 0 0 var(--s-1);
  }

  .team-sub {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--fs-md);
  }

  .team-empty {
    color: var(--text-muted);
  }

  .team-deleted {
    list-style: none;
    margin: 0 0 var(--s-4);
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }

  .team-deleted li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    padding: var(--s-3);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-left: 2px solid var(--moss);
    border-radius: var(--radius);
    font-size: var(--fs-md);
  }

  .team-deleted-actions {
    display: inline-flex;
    gap: var(--s-2);
    flex: none;
  }

  .team-recent {
    margin: 0 0 var(--s-6);
    padding: var(--s-4);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
  }

  .team-recent-title {
    font-family: var(--font-display);
    font-size: var(--fs-lg);
    font-weight: 500;
    letter-spacing: 0.08em;
    margin: 0 0 var(--s-2);
  }

  .team-recent-hint {
    margin: 0 0 var(--s-3);
    max-width: 62ch;
    font-size: var(--fs-sm);
    line-height: 1.5;
    color: var(--text-muted);
  }

  .team-recent-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }

  .team-recent-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    padding: var(--s-2) var(--s-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
  }

  .team-recent-main {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: var(--s-1) var(--s-3);
    min-width: 0;
  }

  .team-recent-name {
    font-family: var(--font-mono);
    font-size: var(--fs-base);
    font-weight: 500;
    overflow-wrap: anywhere;
  }

  .team-recent-when {
    font-size: var(--fs-sm);
    color: var(--text-muted);
  }

  .team-recent-row .btn.is-pending:disabled {
    border-color: var(--vein-dim);
    color: var(--vein-bright);
    animation: team-pending 1.6s var(--ease-out-stone) infinite;
  }

  .team-list {
    list-style: none;
    margin: 0 0 var(--s-6);
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }

  .team-row {
    display: flex;
    justify-content: space-between;
    gap: var(--s-4);
    padding: var(--s-4);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
    transition: border-color var(--dur-default) var(--ease-out-stone);
  }

  .team-row.state-failed {
    border-color: var(--error);
  }

  .team-row-main {
    min-width: 0;
    flex: 1;
  }

  .team-row-title {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--s-2);
  }

  .team-agent-link {
    background: none;
    border: none;
    padding: 0;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: var(--fs-base);
    font-weight: 500;
    cursor: pointer;
    text-decoration: underline;
    text-decoration-color: var(--vein-dim);
    text-underline-offset: 3px;
  }

  .team-agent-link:hover {
    color: var(--vein-bright);
  }

  .team-agent-link:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }

  .team-state {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-muted);
    letter-spacing: 0.04em;
  }

  .team-row.state-failed .team-state {
    color: var(--error);
  }

  .team-chip {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    padding: 1px var(--s-2);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text-muted);
  }

  .team-chip-unread {
    border-color: var(--vein-dim);
    color: var(--vein-bright);
  }

  .team-role {
    margin: var(--s-2) 0 0;
    color: var(--text-muted);
    font-size: var(--fs-md);
    overflow-wrap: anywhere;
  }

  .team-error {
    margin: var(--s-2) 0 0;
    color: var(--error);
    font-size: var(--fs-md);
    overflow-wrap: anywhere;
  }

  .team-error-at {
    color: var(--text-dim);
    margin-left: var(--s-2);
  }

  .team-row-controls {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--s-3);
    flex: none;
  }

  .team-row-settings {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: var(--s-2) var(--s-4);
  }

  .team-autostart,
  .team-visibility-select {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    font-size: var(--fs-sm);
    color: var(--text-muted);
    cursor: pointer;
  }

  .team-visibility-select {
    cursor: default;
  }

  .team-visibility-hint {
    margin: calc(var(--s-3) * -1) 0 var(--s-4);
    max-width: 62ch;
    font-size: var(--fs-sm);
    line-height: 1.5;
    color: var(--text-muted);
  }

  .team-visibility-hint strong {
    font-weight: 500;
    color: var(--text);
  }

  .team-buttons {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--s-2);
  }

  /* An action that doesn't apply reads as absent, not as a paler enabled button. */
  .team-buttons .btn:disabled {
    background: transparent;
    border-style: dashed;
    border-color: var(--border-subtle);
    color: var(--text-dim);
    opacity: 0.7;
  }

  /* The action in flight keeps its place and breathes while it waits. */
  .team-buttons .btn.is-pending:disabled {
    border-style: solid;
    border-color: var(--vein-dim);
    color: var(--vein-bright);
    opacity: 1;
    animation: team-pending 1.6s var(--ease-out-stone) infinite;
  }

  @keyframes team-pending {
    50% {
      opacity: 0.55;
    }
  }

  .team-create {
    padding: var(--s-4);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
  }

  .team-create-title {
    font-family: var(--font-display);
    font-size: var(--fs-lg);
    font-weight: 500;
    letter-spacing: 0.08em;
    margin: 0 0 var(--s-4);
  }

  .team-hint-error {
    color: var(--error);
  }

  .team-visibility {
    border: none;
    padding: 0;
    margin: 0 0 var(--s-4);
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
  }

  .team-visibility legend {
    font-size: var(--fs-sm);
    font-weight: 500;
    color: var(--text-muted);
    margin-bottom: var(--s-2);
    padding: 0;
  }

  .team-visibility label {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: var(--s-2);
    align-items: start;
    padding: var(--s-2) var(--s-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
    font-size: var(--fs-md);
    line-height: 1.4;
    cursor: pointer;
    transition: border-color var(--dur-default) var(--ease-out-stone);
  }

  .team-visibility label:hover,
  .team-visibility label:has(input:checked) {
    border-color: var(--vein-dim);
  }

  .team-visibility input {
    accent-color: var(--vein);
    margin-top: 3px;
  }

  .team-visibility .option-hint {
    grid-column: 2;
    font-size: var(--fs-sm);
    color: var(--text-muted);
  }

  .team-create-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--s-3);
  }

  .team-created {
    font-size: var(--fs-md);
    color: var(--moss-hover);
  }

  @media (max-width: 600px) {
    .team-view {
      padding: var(--s-4) var(--s-3);
    }

    .team-row {
      flex-direction: column;
    }

    .team-row-controls {
      align-items: stretch;
    }

    .team-row-settings {
      justify-content: space-between;
    }

    .team-buttons {
      display: grid;
      grid-template-columns: repeat(4, 1fr);
    }

    .team-buttons .btn {
      min-height: 40px;
      padding-inline: var(--s-2);
    }

    .team-create-actions .btn {
      width: 100%;
      min-height: 44px;
    }

    .team-deleted li {
      flex-direction: column;
      align-items: flex-start;
    }

    .team-recent-row {
      flex-direction: column;
      align-items: stretch;
    }

    .team-recent-row .btn {
      min-height: 40px;
    }
  }
</style>
