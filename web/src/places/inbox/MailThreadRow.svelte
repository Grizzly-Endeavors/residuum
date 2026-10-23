<script lang="ts">
  import type { MailThreadSummary } from "../../lib/hub-types";
  import { Icon } from "../../lib/icons";
  import { displaySubject } from "../../lib/mail-subject";
  import { relativeTime } from "../../lib/time";
  import { VisuallyHidden } from "../../lib/ui";
  import { participantsLabel } from "./mail-model";

  // One conversation in the Inbox's list: who it is with, when it last moved,
  // its subject and the latest message's first line, with an unread mark and
  // a paperclip when it carries files. The row is a link to the thread, so it
  // also opens in a new tab.

  interface Props {
    summary: MailThreadSummary;
    /** The thread's address in the app. */
    href: string;
    /** The thread is the one open beside the list. */
    selected: boolean;
    /** The clock the relative time reads. */
    now: number;
    /** A plain click: open the thread in place. */
    onopen: (event: MouseEvent) => void;
  }

  let { summary, href, selected, now, onopen }: Props = $props();

  const subject = $derived(displaySubject(summary.subject));
  const when = $derived(new Date(summary.latest_at));
</script>

<a
  class="mail-row"
  id="mail-row-{summary.thread_id}"
  {href}
  aria-current={selected ? "true" : undefined}
  data-unread={summary.unread || undefined}
  onclick={onopen}
>
  <span class="mail-row-mark" aria-hidden="true"></span>
  <span class="mail-row-text">
    <span class="mail-row-top">
      <span class="mail-row-who">
        {participantsLabel(summary)}
        {#if summary.message_count > 1}
          <span class="mail-row-count">{summary.message_count}</span>
          <VisuallyHidden>messages</VisuallyHidden>
        {/if}
      </span>
      {#if summary.has_attachments}
        <span class="mail-row-clip">
          <Icon name="paperclip" size={13} />
          <VisuallyHidden>, has attachments</VisuallyHidden>
        </span>
      {/if}
      <time class="mail-row-when" datetime={summary.latest_at} title={when.toLocaleString()}
        >{relativeTime(when, now)}</time
      >
    </span>
    <span class="mail-row-subject">
      {subject}{#if summary.unread}<VisuallyHidden>, unread</VisuallyHidden>{/if}
    </span>
    {#if summary.snippet !== ""}
      <span class="mail-row-snippet">{summary.snippet}</span>
    {/if}
  </span>
</a>

<style>
  .mail-row {
    display: flex;
    gap: var(--space-10);
    padding: var(--space-10) var(--space-12) var(--space-10) var(--space-8);
    border-radius: var(--corner-md);
    color: var(--color-text);
    text-decoration: none;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-1);
    }

    &[aria-current="true"] {
      background: var(--color-vein-tint);
    }

    &:focus-visible {
      outline-offset: calc(-1 * var(--focus-outline-width));
    }
  }

  /* The unread dot keeps its column on read rows, so subjects line up. */
  .mail-row-mark {
    flex: none;
    width: var(--space-8);
    height: var(--space-8);
    margin-top: var(--space-6);
    border-radius: var(--corner-pill);

    .mail-row[data-unread] & {
      background: var(--color-vein-bright);
    }
  }

  .mail-row-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .mail-row-top {
    display: flex;
    align-items: baseline;
    gap: var(--space-8);
    font-size: var(--font-size-sm);
  }

  .mail-row-who {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-2);
    text-overflow: ellipsis;
    white-space: nowrap;

    .mail-row[data-unread] & {
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }

  .mail-row-count {
    margin-left: var(--space-2);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  .mail-row-clip {
    display: inline-flex;
    flex: none;
    align-self: center;
    color: var(--color-text-2);
  }

  .mail-row-when {
    flex: none;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;

    /* text-3 isn't declared on a tint. */
    .mail-row[aria-current="true"] & {
      color: var(--color-text-2);
    }
  }

  .mail-row-subject,
  .mail-row-snippet {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mail-row-subject {
    color: var(--color-text);

    .mail-row[data-unread] & {
      font-weight: var(--font-weight-semibold);
    }

    .mail-row[aria-current="true"] & {
      color: var(--color-vein-bright);
    }
  }

  .mail-row-snippet {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  @media (max-width: 760px) {
    .mail-row {
      padding-block: var(--space-12);
    }
  }
</style>
