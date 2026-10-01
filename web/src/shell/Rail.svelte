<script lang="ts">
  import type { Attachment } from "svelte/attachments";
  import { Icon, type IconName } from "../lib/icons";
  import { hub } from "../lib/hub.svelte";
  import { userInbox } from "../lib/inbox.svelte";
  import { ws } from "../lib/ws.svelte";
  import { router } from "../lib/router.svelte";
  import { formatLocation, HOME, locationAt, placesEqual, type Place } from "../lib/routes";
  import {
    Badge,
    Button,
    IconButton,
    Menu,
    MenuItem,
    MenuSeparator,
    StatusDot,
    VisuallyHidden,
  } from "../lib/ui";
  import type { RailAccordion } from "./accordion.svelte";
  import { AGENT_PLACES, agentRowStatus, homeAttentionCount } from "./rail-model";
  import type { ShellActions } from "./shell-actions";

  // The rail: Home and Inbox, every agent with its places in an accordion,
  // the team's places, and a footer with help and Settings. At phone width the
  // same rail opens in the drawer.

  interface Props {
    accordion: RailAccordion;
    actions: ShellActions;
    /** Set in the phone drawer: shows a close button, and is called once a place opens. */
    onclose?: () => void;
  }

  let { accordion, actions, onclose }: Props = $props();

  const uid = $props.id();
  const INBOX: Place = { kind: "inbox", agent: null, tab: "active", item: null };
  const WORKBENCH: Place = { kind: "workbench", artifact: null };
  const SHARED_FILES: Place = { kind: "shared-files" };
  const ROW_KEYS = new Set(["ArrowDown", "ArrowUp", "Home", "End"]);

  const place = $derived(router.place);
  const homeCount = $derived(homeAttentionCount(hub.agents));

  /** The Inbox and the Workbench stay current whatever filter or artifact they show. */
  function isCurrent(target: Place): boolean {
    if (target.kind === "inbox" || target.kind === "workbench") return place.kind === target.kind;
    return placesEqual(place, target);
  }

  function open(event: MouseEvent, target: Place): void {
    // A modified click opens the place in a new tab, as a link does.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
      return;
    }
    event.preventDefault();
    void router.openPlace(target);
    onclose?.();
  }

  /** The bound agent's running sessions; the rail knows no other agent's yet. */
  function runningCount(agent: string): number {
    if (ws.agent !== agent) return 0;
    return ws.sessions.live.length + ws.sessions.outbound.length;
  }

  /** Up and Down move between the rows that show, Home and End go to the ends. */
  const rowKeys: Attachment<HTMLElement> = (container) => {
    const onKeydown = (event: KeyboardEvent): void => {
      if (!ROW_KEYS.has(event.key) || !(event.target instanceof HTMLElement)) return;
      const rows = [...container.querySelectorAll<HTMLElement>("[data-rail-row]")].filter(
        (row) => row.closest("[hidden]") === null,
      );
      const at = rows.indexOf(event.target);
      if (at < 0) return;
      event.preventDefault();
      const targets: Record<string, number> = {
        Home: 0,
        End: rows.length - 1,
        ArrowDown: Math.min(at + 1, rows.length - 1),
        ArrowUp: Math.max(at - 1, 0),
      };
      rows[targets[event.key] ?? at]?.focus();
    };
    container.addEventListener("keydown", onKeydown);
    return () => container.removeEventListener("keydown", onKeydown);
  };
</script>

{#snippet placeRow(target: Place, label: string, icon: IconName, count = 0, countLabel = "")}
  <li>
    <a
      class="rail-row"
      href={formatLocation(locationAt(target))}
      data-rail-row
      aria-current={isCurrent(target) ? "page" : undefined}
      onclick={(event) => open(event, target)}
    >
      <Icon name={icon} size={15} />
      <span class="rail-label">{label}</span>
      <Badge {count} label={countLabel} solid={target.kind === "inbox"} />
    </a>
  </li>
{/snippet}

<nav class="shell-rail" aria-label="Places and agents">
  <div class="rail-top">
    <span class="rail-mark"><Icon name="mark" size={16} />Residuum</span>
    {#if onclose}
      <IconButton icon="close" label="Close menu" data-overlay-close onclick={onclose} />
    {/if}
  </div>

  <div class="rail-scroll" {@attach rowKeys}>
    <ul class="rail-list rail-home">
      {@render placeRow(
        HOME,
        "Home",
        "home",
        homeCount,
        homeCount === 1 ? "thing needs you" : "things need you",
      )}
      {@render placeRow(INBOX, "Inbox", "inbox", userInbox.unreadCount, "unread")}
    </ul>

    <div class="rail-heading">
      <span id="{uid}-agents">Agents</span>
      <IconButton icon="plus" size="sm" label="Create an agent" onclick={actions.createAgent} />
    </div>
    <ul class="rail-list" aria-labelledby="{uid}-agents">
      {#each hub.agents as agent (agent.name)}
        {@const viewed = router.viewedAgent === agent.name}
        {@const status = agentRowStatus(agent, {
          activity: hub.activityOf(agent.name),
          stopping: hub.isStopping(agent.name),
          viewed,
          onItsChat: viewed && place.kind === "chat",
        })}
        {@const expanded = accordion.open === agent.name}
        <li>
          <button
            type="button"
            class="rail-agent"
            data-rail-row
            data-viewed={viewed || undefined}
            aria-expanded={expanded}
            aria-controls="{uid}-places-{agent.name}"
            onclick={() => accordion.toggle(agent.name)}
          >
            <StatusDot state={status.dot} working={status.working} />
            <span class="rail-label">{agent.name}</span>
            <span class="rail-tail" aria-hidden="true">
              {#if status.tail.kind === "unread"}
                <Badge count={status.tail.count} solid />
              {:else if status.tail.kind === "word"}
                <span class="rail-word" data-tone={status.tail.tone}>{status.tail.word}</span>
              {/if}
            </span>
            <span class="rail-chevron"><Icon name="chevron-down" size={14} /></span>
            <VisuallyHidden>, {status.spoken}</VisuallyHidden>
          </button>
          <ul
            class="rail-places"
            id="{uid}-places-{agent.name}"
            hidden={!expanded}
            data-working={status.working || undefined}
          >
            {#each AGENT_PLACES as entry (entry.kind)}
              {@const target = { kind: entry.kind, agent: agent.name } satisfies Place}
              {@const running = entry.kind === "activity" ? runningCount(agent.name) : 0}
              <li>
                <a
                  class="rail-place"
                  href={formatLocation(locationAt(target))}
                  data-rail-row
                  aria-current={isCurrent(target) ? "page" : undefined}
                  onclick={(event) => open(event, target)}
                >
                  <Icon name={entry.icon} size={15} />
                  <span class="rail-label">{entry.label}</span>
                  <Badge count={running} label="running" />
                </a>
              </li>
            {/each}
          </ul>
        </li>
      {/each}
    </ul>

    <div class="rail-heading"><span id="{uid}-team">Team</span></div>
    <ul class="rail-list" aria-labelledby="{uid}-team">
      {@render placeRow(WORKBENCH, "Workbench", "grid")}
      {@render placeRow(SHARED_FILES, "Shared files", "folder")}
    </ul>
  </div>

  <div class="rail-foot">
    <Menu label="Help" side="top">
      {#snippet trigger(props)}
        <Button variant="quiet" size="sm" icon="help" {...props}>Help</Button>
      {/snippet}
      <MenuItem label="Keyboard shortcuts" onselect={actions.openShortcuts} />
      <MenuItem label="Recent notifications" onselect={actions.openNotifications} />
      <MenuSeparator />
      <MenuItem label="Send feedback" onselect={() => actions.openFeedback("feedback")} />
      <MenuItem label="Report a bug" onselect={() => actions.openFeedback("bug")} />
    </Menu>
    <IconButton icon="settings" label="Settings" onclick={actions.openSettings} />
  </div>
</nav>

<style>
  .shell-rail {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    background: var(--color-stone-1);
  }

  .rail-top {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    min-height: var(--layout-place-header-height);
    padding: var(--space-8) var(--space-10) var(--space-4) var(--space-18);
  }

  .rail-mark {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    font-family: var(--font-mark);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    letter-spacing: 0.2em;
    text-transform: uppercase;
    color: var(--color-text);

    & :global(svg) {
      color: var(--color-vein);
    }
  }

  .rail-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-4) var(--space-8) var(--space-16);
  }

  .rail-list {
    list-style: none;
  }

  .rail-home {
    margin-top: var(--space-4);
  }

  .rail-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-14) var(--space-2) var(--space-6) var(--space-8);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    color: var(--color-text-3);
  }

  .rail-row,
  .rail-agent,
  .rail-place {
    display: flex;
    align-items: center;
    width: 100%;
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
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

  .rail-row,
  .rail-agent {
    gap: var(--space-10);
    height: 32px;
    padding: 0 var(--space-8);
  }

  .rail-agent[data-viewed] {
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .rail-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rail-tail {
    display: contents;
  }

  .rail-word {
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-regular);
    color: var(--color-text-3);

    &[data-tone="danger"] {
      color: var(--color-err-text);
    }

    &[data-tone="accent"] {
      color: var(--color-vein-bright);
    }
  }

  .rail-chevron {
    display: grid;
    color: var(--color-text-3);
    transform: rotate(-90deg);
    transition: transform var(--duration-base) var(--ease-out);

    [aria-expanded="true"] > & {
      transform: none;
    }
  }

  /* An agent's places hang from a thin vein that runs down from its row. */
  .rail-places {
    position: relative;
    padding: var(--space-2) 0 var(--space-8) calc(var(--space-24) + var(--space-2));
    list-style: none;

    &::before {
      content: "";
      position: absolute;
      top: 0;
      bottom: var(--space-14);
      left: 15.5px;
      width: 1px;
      background: linear-gradient(180deg, var(--color-vein-dim), var(--color-vein-tint));
    }

    /* While the agent works, a pulse of light runs down the vein. */
    &[data-working]::after {
      content: "";
      position: absolute;
      top: 0;
      left: 14px;
      width: 4px;
      height: 26px;
      border-radius: var(--space-2);
      background: linear-gradient(180deg, transparent, var(--color-vein-bright), transparent);
      pointer-events: none;
      animation: rail-flow var(--duration-flow) var(--ease-out) infinite;
    }
  }

  .rail-place {
    position: relative;
    gap: 9px;
    height: 30px;
    padding: 0 var(--space-8);
    font-size: var(--font-size-sm);

    &[aria-current="page"]::before {
      content: "";
      position: absolute;
      top: 50%;
      left: -10.5px;
      width: var(--space-8);
      height: 1px;
      background: var(--color-vein-bright);
      box-shadow: 0 0 6px var(--color-vein-line);
    }
  }

  .rail-foot {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-10) var(--space-12) var(--space-14);
    border-top: 1px solid var(--color-line-soft);
  }

  @media (max-width: 760px) {
    .rail-row,
    .rail-agent,
    .rail-place {
      height: var(--layout-touch-target);
    }
  }

  @keyframes rail-flow {
    0% {
      opacity: 0;
      transform: translateY(-8px);
    }

    25% {
      opacity: 1;
    }

    100% {
      opacity: 0;
      transform: translateY(150px);
    }
  }
</style>
