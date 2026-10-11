<script lang="ts">
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { isStoppableState, runStatus } from "../../lib/session-format";
  import { relativeTime } from "../../lib/time";
  import type { ArtifactSummary } from "../../lib/types";
  import { Button, IconButton, Menu, MenuItem, MenuSeparator } from "../../lib/ui";
  import RunStatus from "../activity/RunStatus.svelte";
  import type { WorkbenchList } from "./workbench-list.svelte";
  import { runningWords, type ArtifactRun } from "./workbench-model";

  // One artifact on the Workbench: its title, address and last edit, how many
  // sessions it has running, Open in a new tab, and a menu with Copy link and
  // Delete. Selected, it opens in place to its link and its running sessions,
  // each of which opens in the context panel.

  interface Props {
    item: ArtifactSummary;
    list: WorkbenchList;
    /** Its running sessions, on any agent. */
    runs: ArtifactRun[];
    /** The row's element id, so a link that selects it can scroll to it. */
    id: string;
    open: boolean;
    /** Select or collapse the row. */
    ontoggle: () => void;
    oncopy: () => void;
    onremove: () => void;
    /** The clock the relative times read. */
    now: number;
  }

  let { item, list, runs, id, open, ontoggle, oncopy, onremove, now }: Props = $props();

  const uid = $props.id();
  const url = $derived(list.urlOf(item.name));
  const changing = $derived(list.changing.has(item.name));
  const shown = $derived(router.panel?.kind === "session" ? router.panel : null);

  function openRun({ agent, run }: ArtifactRun): void {
    void router.openPanel({ kind: "session", agent, runId: run.run_id });
  }
</script>

<li class="artifact" {id} data-open={open || undefined} data-changing={changing || undefined}>
  <div class="head">
    <button
      type="button"
      class="head-main"
      aria-expanded={open}
      aria-controls="{uid}-detail"
      onclick={ontoggle}
    >
      <span class="mark"><Icon name="page" size={15} /></span>
      <span class="text">
        <span class="title">{item.title}</span>
        <span class="meta">
          <span class="path">/team/workbench/{item.name}</span>
          {#if changing}
            <span class="when" data-changing>updating now</span>
          {:else}
            <time class="when" datetime={item.modified_at}
              >edited {relativeTime(item.modified_at, now)}</time
            >
          {/if}
          {#if runs.length > 0}<span class="running">{runningWords(runs.length)}</span>{/if}
        </span>
      </span>
    </button>
    <span class="actions">
      {#if url}
        <a
          class="open"
          href={url}
          target="_blank"
          rel="noopener"
          aria-label="Open {item.title}"
          onclick={(event) => {
            list.open(event, item.name);
          }}
        >
          <Icon name="external-link" size={14} />Open
        </a>
      {:else}
        <Button size="sm" icon="external-link" disabled aria-label="Open {item.title}">Open</Button>
      {/if}
      <Menu label="More for {item.title}" align="end">
        {#snippet trigger(props)}
          <IconButton icon="more" label="More for {item.title}" size="sm" {...props} />
        {/snippet}
        <MenuItem icon="copy" label="Copy link" disabled={url === null} onselect={oncopy} />
        <MenuSeparator />
        <MenuItem
          icon="trash"
          label={list.deleting.has(item.name) ? "Deleting…" : "Delete"}
          tone="danger"
          disabled={list.deleting.has(item.name)}
          onselect={onremove}
        />
      </Menu>
    </span>
  </div>

  <div class="detail" id="{uid}-detail" hidden={!open}>
    {#if open}
      {#if url}
        <div class="link">
          <span class="link-text">{url}</span>
          <Button size="sm" variant="quiet" icon="copy" onclick={oncopy}>Copy link</Button>
        </div>
      {/if}

      <section class="runs" aria-labelledby="{uid}-runs">
        <h3 class="runs-heading" id="{uid}-runs">Running sessions</h3>
        {#if runs.length === 0}
          <p class="quiet">Nothing running. Sessions this page starts show here, on any agent.</p>
        {:else}
          <ul class="run-list">
            {#each runs as entry (`${entry.agent}:${entry.run.run_id}`)}
              {@const title = entry.run.purpose || entry.run.address}
              {@const current = shown?.agent === entry.agent && shown.runId === entry.run.run_id}
              <li class="run" data-current={current || undefined}>
                <button
                  type="button"
                  class="run-main"
                  aria-current={current ? "true" : undefined}
                  onclick={() => openRun(entry)}
                >
                  <span class="run-title">{title}</span>
                  <span class="run-sub">
                    <span>On {entry.agent}</span>
                    <RunStatus status={runStatus(entry.run, now)} />
                  </span>
                </button>
                {#if isStoppableState(entry.run.state)}
                  <Button
                    variant="quiet"
                    size="sm"
                    icon="stop"
                    aria-label="Stop {title} on {entry.agent}"
                    loading={list.stopping.has(`${entry.agent}:${entry.run.address}`)}
                    onclick={() => void list.stopRun(entry.agent, entry.run)}>Stop</Button
                  >
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
        <p class="quiet">Finished sessions are in each agent's Activity.</p>
      </section>
    {/if}
  </div>
</li>

<style>
  .artifact {
    border-radius: var(--corner-md);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      box-shadow var(--duration-base) var(--ease-out);

    &[data-open] {
      background: var(--color-stone-1);
    }

    /* The vein seam lights while an agent writes to it. */
    &[data-changing] {
      box-shadow: inset 2px 0 0 var(--color-vein-bright);
    }
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding-right: var(--space-8);
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    .artifact:not([data-open]) > &:hover {
      background: var(--color-stone-1);
    }
  }

  .head-main {
    display: flex;
    flex: 1;
    align-items: center;
    gap: var(--space-12);
    min-width: 0;
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    text-align: left;
  }

  .mark {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    transition:
      background-color var(--duration-base) var(--ease-out),
      color var(--duration-base) var(--ease-out);

    .artifact[data-changing] & {
      background: var(--color-vein-tint);
      color: var(--color-vein-bright);
      animation: artifact-changing var(--duration-pulse) var(--ease-in-out) infinite;
    }
  }

  @keyframes artifact-changing {
    50% {
      box-shadow: 0 0 0 4px var(--color-vein-faint);
    }
  }

  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .title {
    color: var(--color-text);
    overflow-wrap: anywhere;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .path {
    min-width: 0;
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: inherit;
    overflow-wrap: anywhere;
  }

  .when[data-changing] {
    color: var(--color-vein-bright);
  }

  .running {
    color: var(--color-text-2);
  }

  .actions {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-4);
  }

  .open {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    height: 28px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-3);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-4);
    }
  }

  /* The detail lines up with the title, under the row's mark. */
  .detail {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding: 0 var(--space-16) var(--space-16) 54px;

    &[hidden] {
      display: none;
    }
  }

  .link {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);
  }

  .link-text {
    min-width: 0;
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
    user-select: all;
  }

  .runs {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .runs-heading {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  /* The runs' text lines up with the heading; their fill reaches past it. */
  .run-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-inline: calc(-1 * var(--space-8));
    list-style: none;
  }

  .run {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    border-radius: var(--corner-sm);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-2);
    }

    /* The run open in the panel. On the tint, text is only text-2 or vein-bright. */
    &[data-current] {
      background: var(--color-vein-tint);

      & .run-sub,
      & :global(.run-status) {
        color: var(--color-text-2);
      }
    }
  }

  .run-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
    padding: var(--space-6) var(--space-8);
    border-radius: var(--corner-sm);
    text-align: left;
  }

  .run-title {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    overflow-wrap: anywhere;

    .run[data-current] & {
      color: var(--color-vein-bright);
    }
  }

  .run-sub {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .quiet {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  @media (max-width: 760px) {
    .head-main {
      padding: var(--space-12);
    }

    .detail {
      padding: 0 var(--space-12) var(--space-14);
    }

    .open {
      height: var(--layout-touch-target);
    }
  }

  /* Narrow: Open and the menu go under the text, so the title and path keep the width. */
  @container (max-width: 520px) {
    .head {
      flex-wrap: wrap;
      padding-right: 0;
    }

    .head-main {
      flex-basis: 100%;
      padding-bottom: var(--space-6);
    }

    .actions {
      padding: 0 0 var(--space-10) 54px;
    }
  }
</style>
