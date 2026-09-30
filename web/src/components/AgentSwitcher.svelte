<script lang="ts">
  import { tick } from "svelte";
  import AgentStateGlyph from "./AgentStateGlyph.svelte";
  import { hub } from "../lib/hub.svelte";
  import { legacyRouter } from "../lib/legacy-router.svelte";
  import { describeAgent, stateLabel, unreadText } from "../lib/agent-state";
  import type { AgentSummary } from "../lib/hub-types";

  const tipId = "agent-switcher-tip";

  // The agent in the URL may not be in the list yet (or ever); show it anyway
  // so the switcher never claims a different agent than the page is on.
  let entries = $derived.by(() => {
    const listed = hub.agents;
    const current = legacyRouter.agent;
    if (current === null || listed.some((a) => a.name === current)) return listed;
    const unlisted: AgentSummary = {
      name: current,
      state: "stopped",
      last_error: null,
      autostart: false,
      role: null,
      a2a_visibility: "private",
    };
    return [...listed, unlisted];
  });

  // Team pages and hub settings belong to the whole install, so the Team chip
  // is the current place there and no single agent is.
  let onTeamSide = $derived(
    legacyRouter.team !== null ||
      legacyRouter.workbench !== null ||
      legacyRouter.settings?.scope === "hub",
  );

  // A failed agent's last error, shown while its chip is hovered or focused.
  let tipFor = $state<string | null>(null);
  let tipText = $derived.by(() => {
    if (tipFor === null) return null;
    const agent = hub.agent(tipFor);
    if (agent?.state !== "failed") return null;
    return agent.last_error?.message ?? "It stopped without saying why.";
  });

  let nav: HTMLElement | undefined = $state();

  function chips(): HTMLButtonElement[] {
    return nav ? Array.from(nav.querySelectorAll<HTMLButtonElement>(".switcher-chip")) : [];
  }

  // Arrow keys move between chips; Tab leaves the switcher as one stop.
  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && tipFor !== null) {
      tipFor = null;
      return;
    }
    const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!keys.includes(event.key)) return;
    const all = chips();
    const at = all.findIndex((el) => el === document.activeElement);
    if (at === -1) return;
    event.preventDefault();
    let next = at;
    if (event.key === "ArrowLeft") next = (at - 1 + all.length) % all.length;
    if (event.key === "ArrowRight") next = (at + 1) % all.length;
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = all.length - 1;
    all[next]?.focus();
  }

  // Keep the current agent's chip in view when the row scrolls (phone width).
  $effect(() => {
    void legacyRouter.agent;
    void tick().then(() => {
      nav
        ?.querySelector<HTMLElement>(".switcher-chip.current")
        ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    });
  });
</script>

<!-- Arrow keys bubble up from the chip buttons, which are the interactive elements. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<nav class="agent-switcher" aria-label="Agents" bind:this={nav} onkeydown={onKeydown}>
  <ul class="switcher-list">
    {#each entries as agent (agent.name)}
      {@const activity = hub.activityOf(agent.name)}
      {@const current = !onTeamSide && agent.name === legacyRouter.agent}
      <li>
        <button
          type="button"
          class="switcher-chip state-{agent.state}"
          class:current
          aria-current={current ? "true" : undefined}
          aria-label={describeAgent(agent, activity)}
          aria-describedby={tipFor === agent.name && tipText !== null ? tipId : undefined}
          onclick={() => legacyRouter.openAgent(agent.name)}
          onpointerenter={() => {
            tipFor = agent.name;
          }}
          onpointerleave={() => {
            if (tipFor === agent.name) tipFor = null;
          }}
          onfocus={() => {
            tipFor = agent.name;
          }}
          onblur={() => {
            if (tipFor === agent.name) tipFor = null;
          }}
        >
          <AgentStateGlyph state={agent.state} />
          <span class="chip-name" aria-hidden="true">{agent.name}</span>
          {#if agent.state !== "running"}
            <span class="chip-state" aria-hidden="true">{stateLabel(agent.state)}</span>
          {/if}
          {#if activity.busy}
            <span class="chip-busy" aria-hidden="true" title="Working"><i></i><i></i><i></i></span>
          {/if}
          {#if activity.unread > 0}
            <span class="chip-unread" aria-hidden="true">{unreadText(activity.unread)}</span>
          {/if}
        </button>
      </li>
    {/each}
  </ul>
  <button
    type="button"
    class="switcher-chip switcher-team"
    class:current={onTeamSide}
    aria-current={onTeamSide ? "page" : undefined}
    onclick={() => {
      legacyRouter.openTeam("overview");
    }}
  >
    <span class="chip-name">Team</span>
  </button>
  {#if hub.transport.status === "disconnected"}
    <p class="switcher-offline" role="status">
      Agent states may be out of date while the connection is down.
    </p>
  {/if}
  {#if tipText !== null}
    <div id={tipId} class="switcher-tip" role="tooltip">
      <strong>{tipFor} failed.</strong>
      {tipText}
    </div>
  {/if}
</nav>

<style>
  .agent-switcher {
    position: relative;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: 0 var(--s-4);
    min-height: 40px;
    background: rgba(14, 14, 16, 0.6);
    border-bottom: 1px solid var(--border-subtle);
    z-index: 9;
  }

  .switcher-list {
    list-style: none;
    margin: 0;
    padding: var(--s-1) 0;
    display: flex;
    gap: var(--s-2);
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .switcher-list li {
    flex: none;
  }

  .switcher-team {
    flex: none;
  }

  .switcher-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    min-height: 32px;
    padding: 0 var(--s-3);
    background: transparent;
    color: var(--text-muted);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    cursor: pointer;
    white-space: nowrap;
    transition:
      color var(--dur-quick),
      background-color var(--dur-default) var(--ease-out-stone),
      border-color var(--dur-default) var(--ease-out-stone);
  }

  .switcher-chip:hover {
    color: var(--text);
    background: var(--bg-raised);
    border-color: var(--border);
  }

  .switcher-chip:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }

  .switcher-chip.current {
    color: var(--text);
    background: var(--vein-glow);
    border-color: var(--vein-dim);
  }

  .state-failed .chip-state {
    color: var(--error);
  }

  .chip-state {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    letter-spacing: 0.04em;
  }

  .chip-busy {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 10px;
    color: var(--vein-bright);
  }

  .chip-busy i {
    display: block;
    width: 2px;
    height: 100%;
    background: currentColor;
    transform-origin: bottom;
    animation: switcher-pulse 1.4s var(--ease-out-stone) infinite;
  }
  .chip-busy i:nth-child(2) {
    animation-delay: 120ms;
  }
  .chip-busy i:nth-child(3) {
    animation-delay: 240ms;
  }

  .chip-unread {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--vein-dim);
    color: #fff;
    font-size: var(--fs-xs);
    font-weight: 500;
    line-height: 18px;
    text-align: center;
  }

  .switcher-tip,
  .switcher-offline {
    font-size: var(--fs-sm);
  }

  .switcher-offline {
    margin: 0;
    color: var(--text-dim);
    flex: none;
  }

  .switcher-tip {
    position: absolute;
    top: 100%;
    left: var(--s-4);
    right: var(--s-4);
    max-width: 480px;
    margin-top: var(--s-1);
    padding: var(--s-2) var(--s-3);
    background: var(--bg-surface);
    border: 1px solid var(--error);
    border-radius: var(--radius);
    color: var(--text);
    box-shadow: var(--elev-2);
    pointer-events: none;
    z-index: 30;
  }

  @keyframes switcher-pulse {
    0%,
    100% {
      transform: scaleY(0.35);
    }
    50% {
      transform: scaleY(1);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .chip-busy i {
      animation: none;
    }
  }

  @media (max-width: 600px) {
    .agent-switcher {
      padding: 0 var(--s-3);
      min-height: 48px;
    }

    .switcher-chip {
      min-height: 40px;
    }

    /* The row scrolls; fade its right edge so a clipped chip reads as "more". */
    .switcher-list {
      mask-image: linear-gradient(to right, #000 calc(100% - 20px), transparent);
    }

    .switcher-offline {
      display: none;
    }
  }
</style>
