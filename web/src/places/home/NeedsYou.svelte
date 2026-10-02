<script lang="ts">
  import { slide } from "svelte/transition";
  import { failureLine } from "../../lib/agent-failure";
  import { Icon, type IconName } from "../../lib/icons";
  import type { NeedsYouItem } from "../../lib/needs-you";
  import { overview } from "../../lib/overview.svelte";
  import { formatLocation, locationAt, type Place } from "../../lib/routes";
  import { relativeTime } from "../../lib/time";
  import { Badge, Button, Disclosure } from "../../lib/ui";
  import type { ShellActions } from "../../shell/shell-actions";
  import FailedAgentActions from "./FailedAgentActions.svelte";
  import { followLink } from "./follow-link";

  // What needs the user, worst first, each with the fix beside it. An item
  // leaves as soon as the hub reports its condition gone.

  interface Props {
    actions: ShellActions;
    /** The clock the relative times read. */
    now: number;
  }

  let { actions, now }: Props = $props();

  const uid = $props.id();
  const needs = $derived(overview.needsYou);
  const INBOX: Place = { kind: "inbox", agent: null, tab: "active", item: null };

  /** The action in flight on each item, by item key. */
  let pending = $state<Record<string, string | undefined>>({});

  const ICONS: Readonly<Record<NeedsYouItem["kind"], IconName>> = {
    failed: "warning",
    outbound: "handoff",
    inbox: "inbox",
  };

  async function run(key: string, action: string, call: () => Promise<unknown>): Promise<void> {
    if (pending[key] !== undefined) return;
    pending[key] = action;
    try {
      await call();
    } finally {
      pending[key] = undefined;
    }
  }

  function inboxPlace(agent: string, id: string): Place {
    return { kind: "inbox", agent: null, tab: "active", item: { agent, id } };
  }

  /** An item that clears slides away, so the list shows what changed. */
  function leave(node: Element): ReturnType<typeof slide> {
    const token = getComputedStyle(document.documentElement).getPropertyValue("--duration-base");
    return slide(node, { duration: Number.parseFloat(token) || 0 });
  }
</script>

<section class="needs-section" aria-labelledby="{uid}-heading">
  <h2 class="home-heading" id="{uid}-heading">
    Needs you <Badge count={needs.count} label={needs.count === 1 ? "item" : "items"} />
  </h2>

  {#if needs.items.length === 0 && needs.moreInInbox === 0}
    <p class="needs-clear">Nothing is waiting on you.</p>
  {:else}
    <ul class="needs">
      {#each needs.items as item (item.key)}
        <li class="need" data-severity={item.severity} out:leave>
          <span class="need-icon"><Icon name={ICONS[item.kind]} size={15} /></span>
          <div class="need-text">
            {#if item.kind === "failed"}
              <p class="need-title">
                {item.agent} couldn't start
                {#if item.error}<span class="need-meta">{relativeTime(item.error.at, now)}</span
                  >{/if}
              </p>
              <p class="need-detail">{failureLine(item.error?.kind)}</p>
              {#if item.error}
                <Disclosure summary="Details" tone="quiet">
                  <code class="need-reason">{item.error.reason}</code>
                </Disclosure>
              {/if}
            {:else if item.kind === "outbound"}
              <p class="need-title">
                {item.agent} can't reach {item.problem.remote_agent}
                <span class="need-meta">{relativeTime(item.problem.unreachable_since, now)}</span>
              </p>
              <p class="need-detail">
                A task it sent there is waiting, and {item.agent} keeps trying to reach it.
                {#if item.problem.status_text}
                  Last status: {item.problem.status_text}
                {/if}
              </p>
              {@const note = overview.taskNotes[`${item.agent}:${item.problem.task_id}`]}
              {#if note}
                <p class="need-note" role="status">{note} Stop watching closes it here instead.</p>
              {/if}
            {:else}
              <p class="need-title">
                {item.item.title}
                <span class="need-meta">{relativeTime(item.item.at, now)}</span>
              </p>
              <p class="need-detail">In {item.item.agent}'s inbox</p>
            {/if}
          </div>
          <div class="need-actions">
            {#if item.kind === "failed"}
              <FailedAgentActions
                agent={item.agent}
                kind={item.error?.kind ?? "other"}
                size="sm"
                {actions}
              />
            {:else if item.kind === "outbound"}
              {@const busy = pending[item.key]}
              <Button
                size="sm"
                loading={busy === "stop"}
                onclick={() =>
                  void run(item.key, "stop", () =>
                    overview.stopTask(item.agent, item.problem.task_id),
                  )}>Stop task</Button
              >
              <Button
                variant="quiet"
                size="sm"
                loading={busy === "unwatch"}
                onclick={() =>
                  void run(item.key, "unwatch", () =>
                    overview.stopWatching(item.agent, item.problem.task_id),
                  )}>Stop watching</Button
              >
            {:else}
              {@const place = inboxPlace(item.item.agent, item.item.id)}
              <a
                class="need-link"
                href={formatLocation(locationAt(place))}
                aria-label="Open {item.item.title}"
                onclick={(event) => followLink(event, place)}>Open</a
              >
            {/if}
          </div>
        </li>
      {/each}
    </ul>
    {#if needs.moreInInbox > 0}
      <a
        class="needs-more"
        href={formatLocation(locationAt(INBOX))}
        onclick={(event) => followLink(event, INBOX)}
      >
        {needs.moreInInbox}
        {needs.items.some((item) => item.kind === "inbox") ? "more" : "unread"} in Inbox
      </a>
    {/if}
  {/if}

  {#if overview.unreadItemsError}
    <p class="needs-problem" role="alert">
      {overview.unreadItemsError}
      <Button variant="quiet" size="sm" onclick={() => void overview.refreshUnreadItems()}
        >Try again</Button
      >
    </p>
  {/if}
</section>

<style>
  .needs {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    list-style: none;
  }

  .need {
    display: grid;
    grid-template-columns: 30px minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-12);
    padding: var(--space-12) var(--space-14) var(--space-12) var(--space-12);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
  }

  .need-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    align-self: start;
    border-radius: var(--corner-md);
    background: var(--color-err-tint);
    color: var(--color-err-text);
  }

  .need[data-severity="info"] .need-icon {
    background: var(--color-vein-tint);
    color: var(--color-vein-bright);
  }

  .need-text {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-2);
    min-width: 0;
  }

  .need-title {
    font-weight: var(--font-weight-medium);
    overflow-wrap: anywhere;
  }

  .need-meta {
    margin-left: var(--space-8);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-regular);
    font-variant-numeric: tabular-nums;
  }

  .need-detail {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .need-note {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .need-reason {
    display: block;
    padding: var(--space-4) var(--space-8) var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }

  .need-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-6);
  }

  .need-link,
  .needs-more {
    display: inline-flex;
    align-items: center;
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);
  }

  .need-link {
    height: 28px;
    padding: 0 var(--space-12);
    background: var(--color-stone-3);
    color: var(--color-text);

    &:hover {
      background: var(--color-stone-4);
    }
  }

  .needs-more {
    margin-top: var(--space-6);
    padding: var(--space-6) var(--space-8);
    color: var(--color-vein-bright);

    &:hover {
      background: var(--color-stone-1);
    }
  }

  .needs-clear {
    padding: var(--space-12) var(--space-14);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
    color: var(--color-text-2);
  }

  .needs-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    margin-top: var(--space-8);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  @media (max-width: 760px) {
    .need {
      grid-template-columns: 30px minmax(0, 1fr);
    }

    .need-actions {
      grid-column: 2;
      justify-content: flex-start;
    }

    .need-link {
      height: var(--layout-touch-target);
    }
  }
</style>
