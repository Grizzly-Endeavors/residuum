<script lang="ts">
  import { hub } from "../../lib/hub.svelte";
  import { Badge, Button, StatusDot } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import type { ShellActions } from "../../shell/shell-actions";
  import AcrossTheTeam from "./AcrossTheTeam.svelte";
  import AgentBoard from "./AgentBoard.svelte";
  import ComingUp from "./ComingUp.svelte";
  import { teamTally } from "./home-model";
  import NeedsYou from "./NeedsYou.svelte";
  import RecentlyDeleted from "./RecentlyDeleted.svelte";

  // Home: what needs the user, every agent at a glance with its menu, the
  // agents that can be restored, and on the right what happened across the
  // team and what runs next. Below 1180px the right column follows the board.

  let { actions }: { actions: ShellActions } = $props();

  const uid = $props.id();
  const tally = $derived(teamTally(hub.agents));

  /** The clock Home's times read: each second while an agent is working, so its timer moves. */
  let now = $state(Date.now());
  const anyBusy = $derived(hub.agents.some((agent) => hub.activityOf(agent.name).busy));
  $effect(() => {
    now = Date.now();
    const timer = window.setInterval(
      () => {
        now = Date.now();
      },
      anyBusy ? 1000 : 30_000,
    );
    return () => window.clearInterval(timer);
  });
</script>

{#snippet tallyLine()}
  <span class="tally">
    {#if tally.running > 0}
      <span class="tally-part" data-state="running">
        <StatusDot state="running" />{tally.running} running
      </span>
    {/if}
    {#if tally.stopped > 0}
      <span class="tally-part" data-state="stopped">
        <StatusDot state="stopped" />{tally.stopped} stopped
      </span>
    {/if}
    {#if tally.failed > 0}
      <span class="tally-part" data-state="failed">
        <StatusDot state="failed" />{tally.failed} can't start
      </span>
    {/if}
  </span>
{/snippet}

<PlaceHeader title="Home">
  <span class="tally-head">{@render tallyLine()}</span>
</PlaceHeader>

<div class="home-scroll">
  <div class="home">
    <div class="home-main">
      <p class="tally-phone">{@render tallyLine()}</p>

      <NeedsYou {actions} {now} />

      <section aria-labelledby="{uid}-agents">
        <div class="home-heading-row">
          <h2 class="home-heading" id="{uid}-agents">
            Agents <Badge count={hub.agents.length} label="agents" />
          </h2>
          <Button variant="quiet" size="sm" icon="plus" onclick={actions.createAgent}
            >New agent</Button
          >
        </div>
        <AgentBoard {now} />
        <RecentlyDeleted {now} />
      </section>
    </div>

    <aside class="home-aside" aria-label="Team activity">
      <AcrossTheTeam {now} />
      <ComingUp {now} />
    </aside>
  </div>
</div>

<style>
  .home-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .home {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    align-items: start;
    gap: var(--space-40);
    width: 100%;
    max-width: var(--layout-home-width);
    margin-inline: auto;
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  .home-main,
  .home-aside {
    display: flex;
    flex-direction: column;
    gap: var(--space-32);
    min-width: 0;
  }

  .home-aside {
    gap: var(--space-24);
  }

  /* Every section on Home heads itself the same way. */
  .home :global(.home-heading) {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    margin-bottom: var(--space-10);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .home-heading-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    margin-bottom: var(--space-10);

    & .home-heading {
      margin-bottom: 0;
    }
  }

  .tally {
    display: inline-flex;
    flex-wrap: wrap;
    gap: var(--space-14);
    font-size: var(--font-size-sm);
  }

  .tally-head {
    align-self: center;
  }

  .tally-part {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);

    &[data-state="running"] {
      color: var(--color-moss-text);
    }

    &[data-state="stopped"] {
      color: var(--color-text-3);
    }

    &[data-state="failed"] {
      color: var(--color-err-text);
    }
  }

  .tally-phone {
    display: none;
  }

  @media (max-width: 1180px) {
    .home {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-32);
    }
  }

  @media (max-width: 760px) {
    .home {
      padding: var(--space-18) var(--space-16) var(--space-40);
    }

    .home-main {
      gap: var(--space-24);
    }

    .tally-head {
      display: none;
    }

    .tally-phone {
      display: block;
      margin-bottom: calc(var(--space-8) * -1);
    }
  }
</style>
