<script lang="ts">
  import { tick, untrack } from "svelte";
  import { fetchA2aAgents, fetchA2aAgentsRaw, putA2aAgentsRaw } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { hub } from "../../lib/hub.svelte";
  import { toast } from "../../lib/toast.svelte";
  import type { A2aAgentStatus, A2aRemoteAgent, ValidateResponse } from "../../lib/types";
  import {
    Badge,
    Banner,
    Button,
    EmptyState,
    Skeleton,
    TextField,
    type BadgeTone,
  } from "../../lib/ui";
  import RunningOnly from "./RunningOnly.svelte";

  // The agents this one can hand work to. While it runs, the running agent's
  // list: each with whether it can be reached and what its card says, and
  // the user's other installs' agents found through the relay. Otherwise the
  // list in `config/a2a.json`. The file's editor works either way and saves
  // at once.

  let { agent }: { agent: string } = $props();

  type Row = Pick<A2aRemoteAgent, "name" | "url" | "source" | "error" | "card"> & {
    /** Null when read from the file, which can't say. */
    status: A2aAgentStatus | null;
  };

  const STATUS: Readonly<Record<A2aAgentStatus, { label: string; tone: BadgeTone }>> = {
    ok: { label: "Reachable", tone: "positive" },
    error: { label: "Can't reach it", tone: "danger" },
    pending: { label: "Checking", tone: "neutral" },
  };

  const running = $derived(hub.agent(agent)?.state === "running" && !hub.isStopping(agent));

  let rows = $state.raw<Row[] | null>(null);
  let loadError = $state<string | null>(null);
  /** The file doesn't read as JSON, so its list can't be shown. */
  let unreadable = $state(false);

  let editing = $state(false);
  let text = $state("");
  let saving = $state(false);
  let problems = $state.raw<string[]>([]);
  let editor = $state<HTMLInputElement | HTMLTextAreaElement>();

  /** The agents `config/a2a.json` lists, or null when it doesn't read as JSON. */
  function listedInFile(raw: string): Row[] | null {
    if (raw.trim() === "") return [];
    try {
      const doc = JSON.parse(raw) as { agents?: Record<string, { url?: unknown } | null> };
      return Object.entries(doc.agents ?? {}).map(([name, entry]) => ({
        name,
        url: typeof entry?.url === "string" ? entry.url : "",
        source: "config",
        status: null,
        error: null,
        card: null,
      }));
    } catch {
      return null;
    }
  }

  async function load(live: boolean): Promise<void> {
    loadError = null;
    try {
      if (live) {
        rows = await fetchA2aAgents(agent);
        unreadable = false;
      } else {
        const listed = listedInFile(await fetchA2aAgentsRaw(agent));
        unreadable = listed === null;
        rows = listed ?? [];
      }
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't read its remote agents." });
    }
  }

  $effect(() => {
    const live = running;
    untrack(() => void load(live));
  });

  async function edit(): Promise<void> {
    try {
      text = await fetchA2aAgentsRaw(agent);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: "Couldn't read a2a.json." }));
      return;
    }
    problems = [];
    editing = true;
    await tick();
    editor?.focus();
  }

  function problemsOf(result: ValidateResponse): string[] {
    if (result.valid) return [];
    const found = (result.diagnostics ?? []).map((diagnostic) => diagnostic.message);
    if (found.length > 0) return found;
    return [result.error ?? "Some of it can't be used."];
  }

  async function save(): Promise<void> {
    saving = true;
    try {
      problems = problemsOf(await putA2aAgentsRaw(agent, text));
      if (problems.length === 0) {
        editing = false;
        toast.success("Saved a2a.json.");
      }
      await load(running);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: "Couldn't save a2a.json." }));
    } finally {
      saving = false;
    }
  }
</script>

<RunningOnly {agent} subject="whether they can be reached" />
{#if loadError !== null}
  <Banner tone="error">
    {loadError}
    {#snippet actions()}
      <Button size="sm" onclick={() => void load(running)}>Try again</Button>
    {/snippet}
  </Banner>
{:else if rows === null}
  <Skeleton lines={2} label="Loading its remote agents" />
{:else if unreadable}
  <Banner tone="warn"
    >a2a.json doesn't read as JSON, so its list can't be shown. Fix it below.</Banner
  >
{:else if rows.length === 0}
  <EmptyState>No remote agents yet. List one in a2a.json.</EmptyState>
{:else}
  <ul class="agents" aria-label="Remote agents">
    {#each rows as row (row.name)}
      <li class="agent">
        <div class="agent-head">
          <span class="agent-name">{row.name}</span>
          {#if row.source === "sibling"}<Badge>Your other install</Badge>{/if}
          {#if row.status !== null}
            <Badge tone={STATUS[row.status].tone} dot>{STATUS[row.status].label}</Badge>
          {/if}
        </div>
        <code class="agent-url">{row.url}</code>
        {#if row.status === "error" && row.error}<p class="agent-error">{row.error}</p>{/if}
        {#if row.card}
          <p class="agent-desc">{row.card.description}</p>
          {#if row.card.skills.length > 0}
            <p class="agent-skills">
              Skills: {row.card.skills.map((skill) => skill.name).join(", ")}
            </p>
          {/if}
        {/if}
      </li>
    {/each}
  </ul>
{/if}

{#if editing}
  <form
    class="editor"
    aria-label="Edit a2a.json"
    onsubmit={(event) => {
      event.preventDefault();
      void save();
    }}
  >
    <TextField
      label="Contents of a2a.json"
      multiline
      rows={10}
      code
      spellcheck="false"
      bind:value={text}
      bind:element={editor}
      hint={'List each agent by name under "agents", with its "url" and any "headers". ${agent-key:name} in a header reads a key from Saved keys.'}
    />
    {#if problems.length > 0}
      <Banner tone="warn" title="Saved, but some of it can't be used.">
        {problems.join(" ")}
      </Banner>
    {/if}
    <div class="actions">
      <Button variant="quiet" disabled={saving} onclick={() => (editing = false)}>Close</Button>
      <Button type="submit" variant="primary" loading={saving}>Save a2a.json</Button>
    </div>
  </form>
{:else}
  <div>
    <Button icon="edit" onclick={() => void edit()}>Edit a2a.json</Button>
  </div>
{/if}

<style>
  .agents {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .agent {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-10) 0;

    & + & {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .agent-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
  }

  .agent-name {
    font-weight: var(--font-weight-medium);
  }

  .agent-url,
  .agent-skills {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }

  .agent-desc {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .agent-error {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .editor {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
