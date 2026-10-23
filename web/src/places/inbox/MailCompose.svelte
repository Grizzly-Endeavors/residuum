<script lang="ts">
  import { untrack } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { mail } from "../../lib/mail.svelte";
  import { toast } from "../../lib/toast.svelte";
  import { Button, IconButton, TextField } from "../../lib/ui";
  import AttachFiles from "./AttachFiles.svelte";
  import DraftAttachments from "./DraftAttachments.svelte";
  import { deliveryHint, joinNames } from "./thread-model";

  // New message, shown as a thread not sent yet: who it goes to (one agent
  // or more), a subject, the Markdown body and attachments. The draft is the
  // store's, so it outlives this view while the address keeps it open.
  // Cancel asks the place to close it; the place's guard asks about text.

  interface Props {
    /** Recipients to start with, when the draft isn't open already. */
    to: string[];
    onsent: (threadId: string) => void;
    oncancel: () => void;
  }

  let { to, onsent, oncancel }: Props = $props();

  const uid = $props.id();
  const titleId = `${uid}-title`;

  // Opened once: a draft already being written keeps its own recipients.
  const draft = untrack(() => mail.openNewMessage(to));

  /** Every agent, and any recipient the draft holds that the list doesn't (yet). */
  const choices = $derived([
    ...hub.agents.map((agent) => ({ name: agent.name, stopped: agent.state === "stopped" })),
    ...draft.to
      .filter((name) => !hub.agents.some((agent) => agent.name === name))
      .map((name) => ({ name, stopped: false })),
  ]);

  const subject = $derived(draft.subject.trim());

  function toggle(name: string): void {
    draft.to = draft.to.includes(name)
      ? draft.to.filter((held) => held !== name)
      : [...draft.to, name];
    if (draft.sendError !== null && draft.to.length > 0) draft.sendError = null;
  }

  async function send(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const recipients = [...draft.to];
    const threadId = await mail.sendNewMessage();
    if (threadId === null) return;
    toast.success(`Sent to ${joinNames(recipients)}.`);
    onsent(threadId);
  }
</script>

<form class="mail-compose" aria-labelledby={titleId} onsubmit={(event) => void send(event)}>
  <header class="compose-head">
    <IconButton
      class="compose-back"
      icon="back"
      label="Back to the list"
      variant="quiet"
      size="sm"
      onclick={oncancel}
    />
    <div class="compose-title">
      <h2 id={titleId} tabindex="-1">{subject === "" ? "New message" : subject}</h2>
      <p class="compose-state">Not sent yet</p>
    </div>
  </header>

  <div class="compose-fields">
    <fieldset class="compose-to">
      <legend>To</legend>
      <div class="compose-agents">
        {#each choices as choice (choice.name)}
          {@const chosen = draft.to.includes(choice.name)}
          <Button
            size="sm"
            pressed={chosen}
            icon={chosen ? "check" : undefined}
            onclick={() => toggle(choice.name)}
          >
            {choice.name}{#if choice.stopped}<span class="compose-stopped">stopped</span>{/if}
          </Button>
        {:else}
          <p class="compose-none">There are no agents to send it to yet.</p>
        {/each}
      </div>
    </fieldset>
    <TextField label="Subject" placeholder="What is it about?" bind:value={draft.subject} />
    <TextField
      label="Message"
      hint="Markdown works here: lists, links, code."
      multiline
      rows={10}
      placeholder="Write your message…"
      bind:value={draft.body}
    />
    <DraftAttachments {draft} />
  </div>

  {#if draft.sendError !== null}
    <p class="compose-error" role="alert">{draft.sendError}</p>
  {/if}
  <div class="compose-foot">
    <AttachFiles {draft} />
    <span class="compose-hint">
      {draft.sending && draft.uploading ? "Sending once the files finish uploading. " : ""}{draft.to
        .length > 0
        ? deliveryHint(draft.to)
        : "Any file, up to 50 MB each."}
    </span>
    <Button variant="quiet" onclick={oncancel}>Cancel</Button>
    <Button type="submit" variant="primary" icon="send" loading={draft.sending}>Send</Button>
  </div>
</form>

<style>
  /* The place frames it: its padding, reading width and scrolling. */
  .mail-compose {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    min-width: 0;
  }

  /* With the list beside it (the place's split width), the list is the way back. */
  @container mail (min-width: 780px) {
    .mail-compose :global(.compose-back) {
      display: none;
    }
  }

  .compose-head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-12);
  }

  .compose-title {
    flex: 1;
    min-width: 0;

    & h2 {
      font-size: var(--font-size-title);
      font-weight: var(--font-weight-semibold);
      --leading: var(--line-height-tight);
      overflow-wrap: anywhere;
    }
  }

  .compose-state {
    margin-top: var(--space-2);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  /* The fields share one card, as the reply does under a thread. */
  .compose-fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding: var(--space-14) var(--space-16) var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .compose-to {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    min-width: 0;
    border: 0;

    & legend {
      margin-bottom: var(--space-6);
      color: var(--color-text);
      font-size: var(--font-size-sm);
      font-weight: var(--font-weight-medium);
    }
  }

  .compose-agents {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
  }

  .compose-stopped {
    margin-left: var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-regular);
  }

  .compose-none {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .compose-error {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .compose-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-10);
  }

  .compose-hint {
    flex: 1;
    min-width: 120px;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
