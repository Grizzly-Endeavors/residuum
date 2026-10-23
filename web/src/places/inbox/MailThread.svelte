<script lang="ts">
  import { openAgentFileInPanel } from "../../feed/feed-links";
  import Prose from "../../feed/Prose.svelte";
  import type { MailAttachment, MailMessage, MailThread } from "../../lib/hub-types";
  import { Icon } from "../../lib/icons";
  import { displaySubject } from "../../lib/mail-subject";
  import { mail, mailboxOf } from "../../lib/mail.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import { messageTimeLabel } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import { AttachmentChip, Banner, Button, IconButton, Skeleton } from "../../lib/ui";
  import ReplyComposer from "./ReplyComposer.svelte";
  import { joinNames, opensInPanel, seenLine } from "./thread-model";

  // One conversation, open: its subject, Archive or Move to Inbox, every
  // message oldest first with its attachments and, on yours, who has seen
  // it, and the reply to everyone at the foot. The place opens the thread
  // through the store, keeps the address, shows a thread that is gone, and
  // frames this view (its padding, reading width and scrolling).

  interface Props {
    threadId: string;
    onback: () => void;
  }

  let { threadId, onback }: Props = $props();

  // Only the thread this view was given: while the place opens another, the last one doesn't linger.
  const thread = $derived.by(() => {
    const held = mail.thread;
    if (held === null) return null;
    const ours = held.thread_id === threadId || held.messages.some((m) => m.ref === threadId);
    return ours ? held : null;
  });
  const mailbox = $derived(thread === null ? null : mailboxOf(thread));
  const pending = $derived(thread === null ? undefined : mail.pending[thread.thread_id]);
  const problem = $derived(thread === null ? undefined : mail.problems[thread.thread_id]);
  const reply = $derived(mail.openReply);

  function sender(message: MailMessage): string | null {
    return message.from.kind === "agent" ? message.from.agent : null;
  }

  /** Archive or Move to Inbox, back to the list as a mail app does, with the other one as Undo. */
  async function move(open: MailThread, action: "archive" | "restore"): Promise<void> {
    const id = open.thread_id;
    const subject = displaySubject(open.subject);
    const moved = action === "archive" ? await mail.archive(id) : await mail.restore(id);
    if (!moved) return;
    onback();
    const undoAction = action === "archive" ? "restore" : "archive";
    toast.success(
      action === "archive" ? `Archived “${subject}”.` : `Moved “${subject}” to the inbox.`,
      { label: "Undo", onClick: () => void undo(id, undoAction) },
    );
  }

  /** The thread is off screen by now, so a failed Undo is a notification. */
  async function undo(id: string, action: "archive" | "restore"): Promise<void> {
    const undone = action === "archive" ? await mail.archive(id) : await mail.restore(id);
    if (undone) return;
    const reason = mail.problems[id]?.message;
    if (reason !== undefined) notifications.surface("error", reason);
  }

  /** Try a failed action again the way its own control does. */
  async function retry(open: MailThread): Promise<void> {
    const action = problem?.action;
    if (action === "archive" || action === "restore") await move(open, action);
    else await mail.retry(open.thread_id);
  }

  function openAttachment(attachment: MailAttachment): void {
    openAgentFileInPanel(attachment.agent, attachment.path);
  }
</script>

<section class="mail-thread" aria-label="Conversation">
  {#if thread !== null && mail.threadStatus === "ready"}
    {@const open = thread}
    <header class="thread-head">
      <IconButton
        class="thread-back"
        icon="back"
        label="Back to the list"
        variant="quiet"
        size="sm"
        onclick={onback}
      />
      <div class="thread-title">
        <h2 tabindex="-1">{displaySubject(open.subject)}</h2>
        <p class="thread-with">With {joinNames(open.participants)}</p>
      </div>
      {#if mailbox === "inbox"}
        <Button
          size="sm"
          icon="archive"
          loading={pending === "archive"}
          onclick={() => void move(open, "archive")}>Archive</Button
        >
      {:else if mailbox === "archived"}
        <Button
          size="sm"
          icon="restore"
          loading={pending === "restore"}
          onclick={() => void move(open, "restore")}>Move to Inbox</Button
        >
      {/if}
    </header>

    {#if problem !== undefined}
      <Banner tone="error">
        {problem.message}
        {#snippet actions()}
          <Button variant="quiet" size="sm" onclick={() => void retry(open)}>Try again</Button>
        {/snippet}
      </Banner>
    {/if}

    <ol class="thread-messages" aria-label="Messages">
      {#each open.messages as message (message.ref)}
        {@const agent = sender(message)}
        <li>
          <article class="message" data-from={agent === null ? "you" : "agent"}>
            <header class="message-head">
              <span class="message-from">{agent ?? "You"}</span>
              {#if agent === null}
                <span class="message-to">to {joinNames(message.to)}</span>
              {/if}
              <time class="message-when" datetime={message.at}>
                {messageTimeLabel(message.at)}
              </time>
            </header>
            {#if message.body.trim() !== ""}
              <div class="message-body">
                <!-- An agent's paths open its file beside the thread, so the Inbox stays. -->
                <Prose
                  content={message.body}
                  {agent}
                  openPath={agent === null
                    ? undefined
                    : (path) => openAgentFileInPanel(agent, path)}
                />
              </div>
            {/if}
            {#if message.attachments.length > 0}
              <ul class="message-files" aria-label="Attachments">
                {#each message.attachments as attachment (attachment.url)}
                  <li>
                    {#if opensInPanel(attachment)}
                      <AttachmentChip
                        filename={attachment.filename}
                        size={attachment.size}
                        contentType={attachment.content_type}
                        onopen={() => openAttachment(attachment)}
                      />
                    {:else}
                      <AttachmentChip
                        filename={attachment.filename}
                        size={attachment.size}
                        contentType={attachment.content_type}
                        href={attachment.url}
                      />
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
            {#if agent === null && message.seen_by.length > 0}
              <p class="message-seen">
                <span class="seen-marks" aria-hidden="true">
                  {#each message.seen_by as mark (mark.agent)}
                    <span class="seen-mark" data-seen={mark.seen || undefined}>
                      <Icon name={mark.seen ? "check" : "clock"} size={13} />
                    </span>
                  {/each}
                </span>
                {seenLine(message.seen_by)}
              </p>
            {/if}
          </article>
        </li>
      {/each}
    </ol>

    {#if reply !== null && reply.threadId === open.thread_id}
      <ReplyComposer draft={reply} recipients={open.recipients_for_reply} />
    {/if}
  {:else if mail.threadStatus === "error"}
    <header class="thread-head">
      <IconButton
        class="thread-back"
        icon="back"
        label="Back to the list"
        variant="quiet"
        size="sm"
        onclick={onback}
      />
    </header>
    <Banner tone="error">
      {mail.threadError}
      {#snippet actions()}
        <Button variant="quiet" size="sm" onclick={() => void mail.retryOpen()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else}
    <div class="thread-loading">
      <Skeleton shape="text" width="60%" label="Loading the conversation" />
      <Skeleton shape="block" height="140px" />
      <Skeleton shape="block" height="96px" />
    </div>
  {/if}
</section>

<style>
  /* The conversation sits on the page stone; its messages and reply are cards on it. */
  .mail-thread {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    min-width: 0;
  }

  /* With the list beside the thread (the place's split width), the list is the way back. */
  @container mail (min-width: 780px) {
    .mail-thread :global(.thread-back) {
      display: none;
    }
  }

  .thread-head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-12);
  }

  .thread-title {
    flex: 1;
    min-width: 0;

    & h2 {
      font-size: var(--font-size-title);
      font-weight: var(--font-weight-semibold);
      --leading: var(--line-height-tight);
      overflow-wrap: anywhere;
    }
  }

  .thread-with {
    margin-top: var(--space-2);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .thread-messages {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    list-style: none;
  }

  /*
   * An agent's message is a stone-1 card and yours a stone-2 one, named in
   * moss as the chat marks you. Not the chat's moss tint: an attachment
   * chip's control-border ring falls under 3:1 on it.
   */
  .message {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding: var(--space-14) var(--space-16) var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);

    &[data-from="you"] {
      background: var(--color-stone-2);
    }
  }

  .message-head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-2) var(--space-10);
  }

  .message-from {
    color: var(--color-text);
    font-weight: var(--font-weight-semibold);

    [data-from="you"] & {
      color: var(--color-moss-text);
    }
  }

  .message-to {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .message-when {
    margin-left: auto;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  .message-body {
    color: var(--color-text);
  }

  .message-files {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }

  .message-seen {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
  }

  .seen-marks {
    display: inline-flex;
    gap: var(--space-2);
  }

  .seen-mark {
    display: inline-flex;
    color: var(--color-text-3);

    &[data-seen] {
      color: var(--color-moss-text);
    }
  }

  .thread-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }
</style>
