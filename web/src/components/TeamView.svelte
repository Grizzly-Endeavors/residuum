<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { router } from "../lib/router.svelte";
  import { agentNameProblem } from "../lib/agent-name";
  import { stateLabel, unreadText } from "../lib/agent-state";
  import { relativeTime } from "../lib/time";
  import type { A2aVisibility, AgentSummary } from "../lib/hub-types";
  import AgentStateGlyph from "./AgentStateGlyph.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  type Action = "start" | "stop" | "restart" | "autostart" | "visibility" | "delete";

  // What each agent is waiting on right now, so its buttons show progress.
  let pending = $state<Record<string, Action | undefined>>({});

  const PENDING_LABELS: Record<"start" | "stop" | "restart", string> = {
    start: "Starting",
    stop: "Stopping",
    restart: "Restarting",
  };

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

  // ── Delete ──────────────────────────────────────────────────────────

  let confirmDelete = $state<string | null>(null);
  /** Deletions from this visit, with the checkpoint that holds each one's files. */
  let deleted = $state<{ name: string; checkpointId: string | null }[]>([]);

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
        ...(description !== "" ? { description } : {}),
        models_from: modelsFrom,
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
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            onclick={() => {
              deleted = deleted.filter((d) => d.name !== note.name);
            }}>Dismiss</button
          >
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
                  router.openAgent(agent.name);
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
            <label class="team-visibility-select">
              A2A card
              <select
                value={agent.a2a_visibility}
                disabled={busyAction !== undefined}
                aria-describedby="team-visibility-hint"
                onchange={(e) => void changeVisibility(agent, e.currentTarget)}
              >
                <option value="private">Private</option>
                <option value="public">Public</option>
              </select>
            </label>
            <label class="team-autostart">
              <input
                type="checkbox"
                checked={agent.autostart}
                disabled={busyAction !== undefined}
                onchange={(e) => void toggleAutostart(agent, e.currentTarget)}
              />
              Start automatically
            </label>
            <div class="team-buttons">
              <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled={!canStart(agent) || busyAction !== undefined}
                onclick={() => void run(agent.name, "start", () => hub.startAgent(agent.name))}
              >
                {busyAction === "start" ? PENDING_LABELS.start : "Start"}
              </button>
              <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled={!canStop(agent) || busyAction !== undefined}
                onclick={() => void run(agent.name, "stop", () => hub.stopAgent(agent.name))}
              >
                {busyAction === "stop" ? PENDING_LABELS.stop : "Stop"}
              </button>
              <button
                type="button"
                class="btn btn-secondary btn-sm"
                disabled={!canRestart(agent) || busyAction !== undefined}
                onclick={() => void run(agent.name, "restart", () => hub.restartAgent(agent.name))}
              >
                {busyAction === "restart" ? PENDING_LABELS.restart : "Restart"}
              </button>
              <button
                type="button"
                class="btn btn-danger btn-sm"
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

  <p id="team-visibility-hint" class="team-visibility-hint">
    Public shows only an agent's card to other agents. Everything else, including handing it work,
    still needs a caller key.
  </p>

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
        <span class="field-hint">Other agents need a caller key to even see it.</span>
      </label>
      <label>
        <input type="radio" name="create-visibility" value="public" bind:group={visibility} />
        Public
        <span class="field-hint">Anyone who finds the address can see what it can do.</span>
      </label>
    </fieldset>

    <div class="team-create-actions">
      <button type="submit" class="btn btn-primary" disabled={!canCreate}>
        {creating ? "Creating" : "Create agent"}
      </button>
      {#if created}
        <span class="team-created" role="status">
          Created {created}.
          <button
            type="button"
            class="team-agent-link"
            onclick={() => {
              if (created) router.openAgent(created);
            }}>Open it</button
          >
        </span>
      {/if}
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
  checkpoint is taken first, so it can be restored from checkpoints afterwards.

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
    padding: var(--s-5) var(--s-4);
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
    gap: var(--s-3);
    flex: none;
  }

  .team-autostart {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    font-size: var(--fs-sm);
    color: var(--text-muted);
    cursor: pointer;
  }

  .team-visibility-select {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    font-size: var(--fs-sm);
    color: var(--text-muted);
  }

  .team-visibility-hint {
    margin: calc(var(--s-4) * -1) 0 var(--s-6);
    font-size: var(--fs-sm);
    color: var(--text-muted);
  }

  .team-buttons {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--s-2);
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
    color: var(--text-muted);
    margin-bottom: var(--s-2);
    padding: 0;
  }

  .team-visibility label {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: var(--s-2);
    cursor: pointer;
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

    .team-buttons {
      justify-content: flex-start;
    }

    .team-deleted li {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
