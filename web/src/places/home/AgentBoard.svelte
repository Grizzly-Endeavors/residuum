<script lang="ts">
  import { agentLabel } from "../../lib/agent-name";
  import { displayState } from "../../lib/agent-display-state";
  import { hub } from "../../lib/hub.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { formatLocation, locationAt, type Place } from "../../lib/routes";
  import { Badge, Banner, Button, Skeleton, StatusDot, VisuallyHidden } from "../../lib/ui";
  import { lastLine, nowLine, runTitle, runWhen, STATE_WORDS, type RowInput } from "./home-model";
  import AgentMenu from "./AgentMenu.svelte";
  import { followLink } from "./follow-link";

  // Every agent in one aligned table: what it is, its state, what it is doing
  // and said last, how many sessions it runs, what it runs next, and its "…"
  // menu. A row opens the agent's chat. The board's own width picks its
  // layout: Next up goes under 720px, and under 560px, on phones, each row is
  // a card.

  let { now }: { now: number } = $props();
</script>

{#if overview.loadError}
  <div class="board-problem">
    <Banner tone="warn">
      {overview.loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void overview.refreshOverview()}>Try again</Button>
      {/snippet}
    </Banner>
  </div>
{/if}

<div class="board" role="table" aria-label="Agents">
  <div class="board-head" role="rowgroup">
    <div class="board-row" role="row">
      <span role="columnheader">Agent</span>
      <span class="col-state" role="columnheader">State</span>
      <span class="col-now" role="columnheader">Now, and its last message</span>
      <span class="col-run" role="columnheader">Running</span>
      <span class="col-next" role="columnheader">Next up</span>
      <span class="col-menu" role="columnheader"><VisuallyHidden>Manage</VisuallyHidden></span>
    </div>
  </div>
  <div role="rowgroup">
    {#each hub.agents as agent (agent.name)}
      {@const input = {
        agent,
        activity: hub.activityOf(agent.name),
        stopping: hub.isStopping(agent.name),
        overview: overview.overviewOf(agent.name),
        now,
      } satisfies RowInput}
      {@const state = displayState(agent.state, input.stopping)}
      {@const doing = nowLine(input)}
      {@const last = lastLine(input.overview, now)}
      {@const next = input.overview?.upcoming[0]}
      {@const running = input.overview?.live_sessions.length ?? 0}
      {@const chat = { kind: "chat", agent: agent.name } satisfies Place}
      <div class="board-row" role="row">
        <div class="col-agent" role="cell">
          <span class="agent-name">
            <a
              class="agent-link"
              href={formatLocation(locationAt(chat))}
              onclick={(event) => followLink(event, chat)}>{agentLabel(agent)}</a
            >
            <Badge count={input.activity.unread} label="unread" solid />
          </span>
          <span class="agent-role">{agent.role ?? "No role page yet"}</span>
          <span class="agent-card-line">
            <StatusDot {state} working={input.activity.busy} />
            <span class="state-word" data-state={state}>{STATE_WORDS[state]}</span>
            {#if !doing.echo}
              <span class="now" data-tone={doing.tone}>{doing.text}</span>
            {/if}
          </span>
        </div>
        <div class="col-state" role="cell">
          <StatusDot {state} working={input.activity.busy} />
          <span class="state-word" data-state={state}>{STATE_WORDS[state]}</span>
        </div>
        <div class="col-now" role="cell">
          <span class="now" data-tone={doing.tone}>{doing.text}</span>
          {#if last}
            <span class="sub"><span class="when">{last.when}</span>{last.text}</span>
          {:else if !overview.loaded}
            <Skeleton width="70%" />
          {/if}
        </div>
        <div class="col-run" role="cell">
          {#if running > 0}
            {running}
          {:else}
            <span aria-hidden="true">–</span><VisuallyHidden>None</VisuallyHidden>
          {/if}
        </div>
        <div class="col-next" role="cell">
          {#if next}
            <span class="next-title">{runTitle(next.name)}</span>
            <span class="sub">{runWhen(next, state, now)}</span>
          {:else if overview.loaded}
            <span class="quiet">Nothing scheduled</span>
          {:else}
            <Skeleton width="80%" />
          {/if}
        </div>
        <div class="col-menu" role="cell">
          <AgentMenu {agent} {state} working={input.activity.busy} />
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .board-problem {
    margin-bottom: var(--space-10);
  }

  .board {
    container: home-board / inline-size;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .board-row {
    --board-columns: minmax(150px, 1.1fr) 118px minmax(0, 2fr) 60px minmax(120px, 1fr) 32px;

    position: relative;
    display: grid;
    grid-template-columns: var(--board-columns);
    align-items: center;
    gap: var(--space-16);
    padding: var(--space-10) var(--space-4) var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-1);
    }

    &:has(.agent-link:focus-visible) {
      outline: var(--focus-outline-width) solid var(--color-vein-bright);
      outline-offset: calc(var(--focus-outline-width) * -1);
    }
  }

  .board-head .board-row {
    padding-block: var(--space-4) var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    &:hover {
      background: none;
    }
  }

  [role="cell"] {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .agent-name {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-width: 0;
  }

  /* The name's link covers the row, so the whole row opens the chat. */
  .agent-link {
    overflow: hidden;
    color: var(--color-text);
    font-weight: var(--font-weight-semibold);
    text-decoration: none;
    text-overflow: ellipsis;
    white-space: nowrap;

    &::after {
      position: absolute;
      inset: 0;
      border-radius: var(--corner-md);
      content: "";
    }

    &:focus-visible {
      outline: none;
    }
  }

  .agent-role,
  .now,
  .sub {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-role,
  .sub {
    margin-top: var(--space-2);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .agent-card-line {
    display: none;
  }

  .col-state {
    flex-direction: row;
    align-items: center;
    gap: var(--space-4);
  }

  .state-word {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    white-space: nowrap;

    &[data-state="running"] {
      color: var(--color-moss-text);
    }

    &[data-state="starting"] {
      color: var(--color-vein-bright);
    }

    &[data-state="stopped"] {
      color: var(--color-text-3);
    }

    &[data-state="failed"] {
      color: var(--color-err-text);
    }
  }

  .now[data-tone="quiet"],
  .quiet {
    color: var(--color-text-3);
  }

  .now[data-tone="accent"] {
    color: var(--color-vein-bright);
  }

  .now[data-tone="danger"] {
    color: var(--color-err-text);
  }

  .when {
    margin-right: var(--space-6);
    font-variant-numeric: tabular-nums;
  }

  .col-run {
    align-items: flex-end;
    color: var(--color-text-2);
    font-variant-numeric: tabular-nums;
  }

  .board-head .col-run {
    text-align: right;
  }

  .col-next {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-tight);
  }

  /* Positioned after the name's link, so the menu sits over the row's link and takes its own presses. */
  .col-menu {
    position: relative;
    align-items: center;
  }

  @container home-board (max-width: 719px) {
    .board-row {
      --board-columns: minmax(140px, 1fr) 112px minmax(0, 1.6fr) 56px 32px;
    }

    .col-next {
      display: none;
    }
  }

  /* Too narrow for aligned columns, a phone or a narrow window: one card per agent. */
  @container home-board (max-width: 559px) {
    .board-head {
      display: none;
    }

    .board-row {
      --board-columns: minmax(0, 1fr) auto;

      padding: var(--space-10) var(--space-4) var(--space-10) var(--space-10);
    }

    .col-state,
    .col-now,
    .col-run,
    .agent-role {
      display: none;
    }

    .agent-card-line {
      display: flex;
      align-items: center;
      gap: var(--space-4);
      min-width: 0;
      margin-top: var(--space-4);
      font-size: var(--font-size-sm);

      & .now {
        margin-left: var(--space-4);
      }
    }
  }
</style>
