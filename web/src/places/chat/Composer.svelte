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

  interface Props {
    agent: string;
    replying: boolean;
    reconnecting: boolean;
    queued: number;
    onsend: (text: string, attachments: StagedAttachment[]) => boolean;
    onstop: () => void;
  }

  let { agent, replying, reconnecting, queued, onsend, onstop }: Props = $props();

  let text = $state(untrack(() => readDraft(agent)));
  const files = untrack(() => draftAttachments(agent));
  let sendHeld = $state(false);
  let frame = $state<ReturnType<typeof ComposerFrame>>();
  let filePicker = $state<HTMLInputElement>();

  $effect(() => {
    saveDraft(agent, text);
  });

  const heldReason = $derived(sendHeld ? files.problem : null);
  $effect(() => {
    if (files.problem === null) sendHeld = false;
  });

  const menu: ComposerMenu = $derived({
    label: "Chat actions",
    actions: commandActions(actionRegistry.all),
  });

  const stopKey = new PressAgain();

  $effect(() => {
    if (!replying) stopKey.disarm();
  });
  $effect(() => () => {
    stopKey.disarm();
  });

  function onkeys(event: KeyboardEvent, menuShown: boolean): void {
    if (event.isComposing) return;
    if (event.key !== "Escape") {
      stopKey.disarm();
      return;
    }
    if (event.defaultPrevented || menuShown || !replying || overlayOpen()) return;
    event.preventDefault();
    if (event.repeat) return;
    if (stopKey.press()) onstop();
  }

  export function restore(message: string, attachments: readonly StagedAttachment[] = []): void {
    if (message !== "") text = text.trim() === "" ? message : `${message}\n\n${text}`;
    files.restore(attachments);
    frame?.focus();
  }

  export function attach(dropped: Iterable<File>): void {
    files.attach(dropped);
  }

  function onpaste(event: ClipboardEvent): void {
    const pasted = [...(event.clipboardData?.files ?? [])];
    if (pasted.length === 0) return;
    event.preventDefault();
    attach(pasted);
  }

  const empty = $derived(text.trim() === "" && files.attachments.length === 0);
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
