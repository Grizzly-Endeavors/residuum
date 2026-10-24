<script lang="ts">
  import { untrack } from "svelte";
  import ComposerFrame, { type ComposerMenu } from "../../composer/ComposerFrame.svelte";
  import { actionRegistry, commandActions } from "../../lib/action-registry.svelte";
  import StagedFiles from "../../composer/StagedFiles.svelte";
  import { draftAttachments, readDraft, saveDraft } from "../../lib/composer-drafts";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { PressAgain } from "../../lib/press-again.svelte";
  import type { StagedAttachment } from "../../lib/staged-attachments.svelte";
  import { IconButton, overlayOpen } from "../../lib/ui";
  import ModelControl from "./ModelControl.svelte";

  // The chat's composer, on the frame it shares with a session's message box
  // (`ComposerFrame`): files attached by button, paste or drop, each
  // uploading at once, the chat actions under `/`, the model and thinking
  // control, and Send, which becomes Stop while a reply runs and nothing is
  // typed. Send waits while a file is uploading or one failed, and says so.
  // What is typed and attached is kept per agent until it is sent. While the
  // connection is down, what is sent waits for it.

  interface Props {
    agent: string;
    /** A reply is under way. */
    replying: boolean;
    /** The agent's connection is down and coming back. */
    reconnecting: boolean;
    /** Messages waiting for the connection. */
    queued: number;
    /**
     * A message, or a `/name text` line, to send, with the files attached to
     * it, every one uploaded. False when it couldn't go (a command that can't
     * run now): the box keeps what was typed and attached.
     */
    onsend: (text: string, attachments: StagedAttachment[]) => boolean;
    onstop: () => void;
  }

  let { agent, replying, reconnecting, queued, onsend, onstop }: Props = $props();

  let text = $state(untrack(() => readDraft(agent)));
  const files = untrack(() => draftAttachments(agent));
  /** Send was pressed while a file was uploading or had failed, so the composer says why it waits. */
  let sendHeld = $state(false);
  let frame = $state<ReturnType<typeof ComposerFrame>>();
  let filePicker = $state<HTMLInputElement>();

  $effect(() => {
    saveDraft(agent, text);
  });

  // The reason goes once there is nothing left to wait for.
  const heldReason = $derived(sendHeld ? files.problem : null);
  $effect(() => {
    if (files.problem === null) sendHeld = false;
  });

  const menu: ComposerMenu = $derived({
    label: "Chat actions",
    actions: commandActions(actionRegistry.all),
  });

  // ── Stopping with Esc ──────────────────────────────────────────────

  // Esc is also how a dialog or menu closes, so it takes two presses to stop
  // a reply: the first says so and waits for the second.
  const stopKey = new PressAgain();

  $effect(() => {
    if (!replying) stopKey.disarm();
  });
  $effect(() => () => {
    stopKey.disarm();
  });

  // Keys from anywhere in the composer, heard before the message box's own
  // handler: the open `/` menu is checked here rather than by that handler
  // claiming the key. An open overlay keeps the press for closing itself.
  function onkeys(event: KeyboardEvent, menuShown: boolean): void {
    if (event.isComposing) return;
    if (event.key !== "Escape") {
      stopKey.disarm();
      return;
    }
    if (event.defaultPrevented || menuShown || !replying || overlayOpen()) return;
    event.preventDefault();
    // A held key repeats; it is one press.
    if (event.repeat) return;
    if (stopKey.press()) onstop();
  }

  /**
   * Put a message that couldn't go back in the box, ahead of anything typed
   * since, with the files it carried ahead of any attached since.
   */
  export function restore(message: string, attachments: readonly StagedAttachment[] = []): void {
    if (message !== "") text = text.trim() === "" ? message : `${message}\n\n${text}`;
    files.restore(attachments);
    frame?.focus();
  }

  // ── Files ──────────────────────────────────────────────────────────

  /** Attach `dropped`, such as files dropped on the chat: each starts uploading at once. */
  export function attach(dropped: Iterable<File>): void {
    files.attach(dropped);
  }

  function onpaste(event: ClipboardEvent): void {
    const pasted = [...(event.clipboardData?.files ?? [])];
    if (pasted.length === 0) return;
    event.preventDefault();
    attach(pasted);
  }

  // ── Sending ────────────────────────────────────────────────────────

  const empty = $derived(text.trim() === "" && files.attachments.length === 0);
  // With nothing typed while a reply runs, Send is Stop; typing brings Send
  // back, for a message that steers the reply.
  const showStop = $derived(replying && empty);

  function send(): boolean {
    if (empty) return false;
    if (files.problem !== null) {
      sendHeld = true;
      return false;
    }
    const attachments = files.take();
    if (!onsend(text.trim(), attachments)) {
      files.restore(attachments);
      return false;
    }
    text = "";
    sendHeld = false;
    return true;
  }

  function stopFromButton(): void {
    onstop();
    frame?.focus();
  }

  const queuedLine = $derived(
    queued === 0
      ? "Reconnecting — messages you send now will go out once back online."
      : `Reconnecting — ${queued === 1 ? "1 message" : `${String(queued)} messages`} will send once back online.`,
  );
</script>

<ComposerFrame
  bind:this={frame}
  bind:value={text}
  label="Message {hub.shownName(agent)}"
  placeholder="Message {hub.shownName(agent)}"
  {menu}
  onsend={send}
  {onkeys}
  onleave={() => {
    stopKey.disarm();
  }}
  {onpaste}
>
  {#snippet above()}
    {#if reconnecting}
      <p class="composer-line" role="status"><Icon name="wifi-off" size={14} />{queuedLine}</p>
    {/if}
    <StagedFiles {files} />
    {#if heldReason !== null}
      <p class="composer-line composer-problem" role="alert">{heldReason}</p>
    {/if}
  {/snippet}
  {#snippet tools(menuButton)}
    <IconButton icon="paperclip" label="Attach files" onclick={() => filePicker?.click()} />
    {@render menuButton()}
    <ModelControl {agent} />
  {/snippet}
  {#snippet sendControl()}
    {#if stopKey.armed}
      <span class="composer-hint" role="status">Press Esc again to stop</span>
    {/if}
    <!-- One element for Send and Stop, so focus has nothing to fall off when it changes. A press leaves focus in the message box: on a phone that keeps the keyboard up. -->
    <IconButton
      icon={showStop ? "stop" : "send"}
      variant={showStop ? "secondary" : "primary"}
      label={showStop ? "Stop reply" : "Send"}
      type={showStop ? "button" : "submit"}
      disabled={empty && !showStop}
      onmousedown={(event) => event.preventDefault()}
      onclick={showStop ? stopFromButton : undefined}
    />
  {/snippet}
  <input
    bind:this={filePicker}
    type="file"
    multiple
    hidden
    onchange={(event) => {
      const input = event.currentTarget;
      attach([...(input.files ?? [])]);
      // The same file can be picked again.
      input.value = "";
    }}
  />
</ComposerFrame>

<style>
  .composer-line {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);

    & :global(svg) {
      flex: none;
    }
  }

  .composer-problem {
    color: var(--color-err-text);
  }

  .composer-hint {
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
