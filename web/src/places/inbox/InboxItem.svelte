<script lang="ts">
  import { tick } from "svelte";
  import MarkdownContent from "../../components/MarkdownContent.svelte";
  import type { HubInboxItem } from "../../lib/hub-types";
  import { Icon } from "../../lib/icons";
  import { inbox, inboxItemKey } from "../../lib/inbox.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import { formatLocation, locationAt, type Place } from "../../lib/routes";
  import { relativeTime } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import { Button, VisuallyHidden } from "../../lib/ui";
  import { followLink } from "../home/follow-link";
  import { fileSize, sourceLabel } from "./inbox-model";

  // One item in the Inbox: a row with its title, agent, source, time and
  // unread mark, which opens in place to the body, the attachments and what
  // can be done with it.

  interface Props {
    item: HubInboxItem;
    /** The row's element id, so a link that opens the item can scroll to it. */
    id: string;
    open: boolean;
    /** Open or close the item. */
    ontoggle: () => void;
    /** The item left this list while it was open. */
    onleave: () => void;
    /** The clock the relative time reads. */
    now: number;
  }

  let { item, id, open, ontoggle, onleave, now }: Props = $props();

  const uid = $props.id();
  const key = $derived(inboxItemKey(item));
  const source = $derived(sourceLabel(item.source));
  const pending = $derived(inbox.pending[key]);
  const problem = $derived(inbox.problems[key]);
  const archived = $derived(inbox.list?.tab === "archived");
  const chat = $derived<Place>({ kind: "chat", agent: item.agent });

  let row = $state<HTMLLIElement>();

  /** Where focus goes once this row leaves the list: the next row, the one before, or the list. */
  function focusAfterLeaving(): () => void {
    const neighbour = row?.nextElementSibling ?? row?.previousElementSibling;
    const target =
      neighbour?.querySelector<HTMLElement>(".item-head") ??
      row?.closest<HTMLElement>("[role=tabpanel]");
    return () => target?.focus();
  }

  async function move(): Promise<void> {
    const refocus = focusAfterLeaving();
    const moved = archived ? await inbox.restore(item) : await inbox.archive(item);
    if (!moved) return;
    if (open) onleave();
    await tick();
    refocus();
    if (archived) {
      toast.success(`Moved “${item.title}” back to the inbox.`);
    } else {
      toast.success(`Archived “${item.title}”.`, { label: "Undo", onClick: () => void undo() });
    }
  }

  /** Try the failed action again, the way its own button does it. */
  async function retry(): Promise<void> {
    if (problem?.action === "read") await inbox.retry(item);
    else await move();
  }

  /** Bring an archived item back. It isn't on screen any more, so a failure is a notification. */
  async function undo(): Promise<void> {
    if (await inbox.restore(item)) return;
    const reason = inbox.problems[key]?.message;
    if (reason !== undefined) notifications.surface("error", reason);
  }
</script>

<li
  class="item"
  {id}
  bind:this={row}
  data-open={open || undefined}
  data-unread={!item.read || undefined}
>
  <button
    type="button"
    class="item-head"
    aria-expanded={open}
    aria-controls="{uid}-body"
    onclick={ontoggle}
  >
    <span class="item-mark"><Icon name="inbox" size={15} /></span>
    <span class="item-text">
      <span class="item-title">{item.title}</span>
      <span class="item-meta">
        <span class="item-agent">{item.agent}</span>
        {#if source}<span>{source}</span>{/if}
        <time datetime={item.at}>{relativeTime(item.at, now)}</time>
      </span>
    </span>
    {#if !item.read}
      <span class="item-dot" aria-hidden="true"></span>
      <VisuallyHidden>, unread</VisuallyHidden>
    {/if}
  </button>

  <div class="item-body" id="{uid}-body" hidden={!open}>
    {#if open}
      {#if item.body.trim() !== ""}
        <div class="item-prose"><MarkdownContent content={item.body} /></div>
      {/if}
      {#if item.attachments.length > 0}
        <ul class="item-files" aria-label="Attachments">
          {#each item.attachments as attachment (attachment.url)}
            <li>
              <a class="item-file" href={attachment.url} download={attachment.filename}>
                <Icon name="paperclip" size={14} />
                <span class="item-file-name">{attachment.filename}</span>
                <span class="item-file-size">{fileSize(attachment.size)}</span>
              </a>
            </li>
          {/each}
        </ul>
      {/if}
      {#if problem}
        <p class="item-problem" role="alert">
          {problem.message}
          <Button variant="quiet" size="sm" onclick={() => void retry()}>Try again</Button>
        </p>
      {/if}
      <div class="item-actions">
        <Button
          size="sm"
          icon={archived ? "restore" : "archive"}
          loading={pending === "archive" || pending === "restore"}
          onclick={() => void move()}>{archived ? "Move back to inbox" : "Archive"}</Button
        >
        <a
          class="item-link"
          href={formatLocation(locationAt(chat))}
          onclick={(event) => followLink(event, chat)}>Reply to {item.agent}</a
        >
      </div>
    {/if}
  </div>
</li>

<style>
  .item {
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    &[data-open] {
      background: var(--color-stone-1);
    }
  }

  .item-head {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    width: 100%;
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    text-align: left;
    transition: background-color var(--duration-fast) var(--ease-out);

    .item:not([data-open]) > &:hover {
      background: var(--color-stone-1);
    }
  }

  .item-mark {
    display: grid;
    flex: none;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);

    .item[data-unread] & {
      background: var(--color-vein-tint);
      color: var(--color-vein-bright);
    }
  }

  .item-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .item-title {
    color: var(--color-text);
    overflow-wrap: anywhere;

    .item[data-unread] & {
      font-weight: var(--font-weight-semibold);
    }
  }

  .item-meta {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .item-agent {
    color: var(--color-text-2);
  }

  .item-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: var(--corner-pill);
    background: var(--color-vein-bright);
  }

  /* The body lines up with the title, under the row's mark. */
  .item-body {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-12);
    padding: 0 var(--space-16) var(--space-14) 54px;

    &[hidden] {
      display: none;
    }
  }

  .item-prose {
    max-width: var(--layout-reading-width);
    color: var(--color-text-2);
    font-size: var(--font-size-ui);
    line-height: var(--line-height-message);
    overflow-wrap: anywhere;

    & :global(:is(p, ul, ol, pre, blockquote, h1, h2, h3, h4, table) + *) {
      margin-top: var(--space-8);
    }

    & :global(:is(ul, ol)) {
      padding-left: var(--space-20);
    }

    & :global(:is(h1, h2, h3, h4)) {
      color: var(--color-text);
      font-size: var(--font-size-ui);
      font-weight: var(--font-weight-semibold);
    }

    & :global(:is(strong, b)) {
      color: var(--color-text);
      font-weight: var(--font-weight-semibold);
    }

    & :global(pre) {
      padding: var(--space-8) var(--space-10);
      border-radius: var(--corner-sm);
      background: var(--color-stone-2);
      font-size: var(--font-size-xs);
      overflow-x: auto;
    }

    & :global(blockquote) {
      padding-left: var(--space-12);
      border-left: 2px solid var(--color-line);
    }
  }

  .item-files {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }

  .item-file {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    max-width: 100%;
    height: 28px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    text-decoration: none;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
    }
  }

  .item-file-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item-file-size {
    flex: none;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .item-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .item-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-6);
  }

  .item-link {
    display: inline-flex;
    align-items: center;
    height: 28px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
      color: var(--color-text);
    }
  }

  @media (max-width: 760px) {
    .item-head {
      padding: var(--space-12);
    }

    .item-body {
      padding: 0 var(--space-12) var(--space-14);
    }

    .item-file,
    .item-link {
      height: var(--layout-touch-target);
    }
  }
</style>
