<script lang="ts">
  import { displayState } from "../../lib/agent-display-state";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { settingsModel } from "../../lib/settings-model.svelte";
  import {
    ALL_SCOPE,
    scopeKind,
    sectionGroups,
    type SectionEntry,
    type SectionId,
  } from "../../lib/settings-sections";
  import { IconButton, SelectField, StatusDot, VisuallyHidden } from "../../lib/ui";
  import type { SettingsScope } from "./sections";

  // The modal's side: the scope picker ("All agents" first, then each agent
  // with its state), what the scope affects, and the scope's sections with
  // the Advanced group under a heading. On a phone it is its own screen.

  interface Props {
    /** The scope token the URL names: an agent, or `_all`. */
    scopeId: string;
    /** The section shown, marked current. */
    current: SectionId | null;
    /** The loaded scope, whose save problems mark their sections. */
    scope: SettingsScope | null;
    onsection: (section: SectionId) => void;
    onscope: (scopeId: string) => void;
    onclose: () => void;
  }

  let { scopeId, current, scope, onsection, onscope, onclose }: Props = $props();

  const kind = $derived(scopeKind(scopeId));
  const groups = $derived(sectionGroups(kind));
  const agent = $derived(kind === "agent" ? hub.agent(scopeId) : undefined);
  const staged = $derived(new Set(settingsModel.stagedScopes.map((open) => open.id)));
  const options = $derived([
    { value: ALL_SCOPE, label: optionLabel("All agents", ALL_SCOPE, []) },
    ...hub.agents.map((entry) => {
      const notes = entry.state === "failed" ? ["couldn't start"] : [];
      if (entry.state === "stopped") notes.push("stopped");
      return { value: entry.name, label: optionLabel(entry.name, entry.name, notes) };
    }),
  ]);

  function optionLabel(name: string, id: string, notes: string[]): string {
    const all = staged.has(id) ? [...notes, "unsaved"] : notes;
    return all.length === 0 ? name : `${name} (${all.join(", ")})`;
  }

  function hasProblems(section: SectionId): boolean {
    return scope?.diagnostics.some((placed) => placed.section === section) ?? false;
  }
</script>

{#snippet link(entry: SectionEntry)}
  <button
    type="button"
    class="nav-link"
    data-section={entry.id}
    aria-current={entry.id === current ? "page" : undefined}
    onclick={() => onsection(entry.id)}
  >
    <span class="nav-label">
      {entry.label}
      {#if hasProblems(entry.id)}
        <span class="nav-problem"><Icon name="warning" size={13} /></span>
        <VisuallyHidden>, has problems</VisuallyHidden>
      {/if}
    </span>
    <span class="nav-description">{entry.description}</span>
  </button>
{/snippet}

<nav class="settings-nav" aria-label="Settings sections">
  <div class="nav-top">
    <h2 class="nav-title">Settings</h2>
    <IconButton icon="close" label="Close settings" onclick={onclose} />
  </div>
  <div class="scope">
    <span class="scope-mark">
      {#if agent}
        <StatusDot state={displayState(agent.state, hub.isStopping(agent.name))} />
      {:else}
        <Icon name="users" size={15} />
      {/if}
    </span>
    <SelectField
      label="Settings for"
      labelHidden
      value={scopeId}
      {options}
      data-scope-picker
      onchange={(event) => onscope(event.currentTarget.value)}
    />
  </div>
  <p class="scope-hint">{kind === "all" ? "Applies to every agent" : `Only affects ${scopeId}`}</p>
  {#each groups.main as entry (entry.id)}
    {@render link(entry)}
  {/each}
  <p class="nav-group" id="settings-advanced">Advanced</p>
  <div class="nav-links" role="group" aria-labelledby="settings-advanced">
    {#each groups.advanced as entry (entry.id)}
      {@render link(entry)}
    {/each}
  </div>
</nav>

<style>
  .settings-nav {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-16) var(--space-10);
    overflow-y: auto;
    background: var(--color-stone-0);

    & > * {
      flex-shrink: 0;
    }
  }

  .nav-top {
    display: none;
  }

  .scope {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding: 0 var(--space-2) var(--space-2);
  }

  .scope :global(.ui-field) {
    flex: 1;
    min-width: 0;
  }

  .scope :global(select) {
    border-color: transparent;
    background: var(--color-stone-2);
    font-weight: var(--font-weight-semibold);
  }

  .scope-mark {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
  }

  .scope-hint {
    margin: var(--space-4) 0 var(--space-10);
    padding: 0 var(--space-8);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .nav-link {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: var(--space-2);
    width: 100%;
    min-height: 34px;
    padding: var(--space-6) var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    text-align: left;
    transition:
      background var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
      color: var(--color-text);
    }

    &[aria-current="page"] {
      background: var(--color-vein-tint);
      color: var(--color-text);
    }
  }

  .nav-label {
    display: flex;
    align-items: center;
    gap: var(--space-6);
  }

  .nav-problem {
    display: grid;
    color: var(--color-err-text);
  }

  .nav-description {
    display: none;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .nav-group {
    margin: var(--space-16) 0 var(--space-4);
    padding: var(--space-14) var(--space-10) var(--space-2);
    border-top: 1px solid var(--color-line-soft);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
  }

  .nav-links {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  @media (max-width: 760px) {
    .settings-nav {
      padding: var(--space-6) var(--space-10) var(--space-32);
      background: var(--color-stone-1);
    }

    .nav-top {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: var(--space-2) 0 var(--space-10) var(--space-8);
    }

    .nav-title {
      font-size: var(--font-size-heading);
      font-weight: var(--font-weight-semibold);
    }

    .nav-link {
      min-height: 56px;
      font-size: var(--font-size-message);
    }

    .nav-description {
      display: block;
    }

    .nav-group {
      margin-top: var(--space-20);
      padding-top: var(--space-16);
    }
  }
</style>
