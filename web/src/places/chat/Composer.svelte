<script lang="ts">
  import { untrack } from "svelte";
  import {
    actionRegistry,
    commandActions,
    matchActions,
    type AppAction,
  } from "../../lib/action-registry.svelte";
  import {
    readDraft,
    readDraftImages,
    saveDraft,
    saveDraftImages,
  } from "../../lib/composer-drafts";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { IMAGE_TYPES, readImages } from "../../lib/image-attachments";
  import type { ImageAttachment } from "../../lib/types";
  import { IconButton, VisuallyHidden } from "../../lib/ui";
  import ModelControl from "./ModelControl.svelte";
  import SlashMenu from "./SlashMenu.svelte";

  // The composer: a message box that grows with what is typed,
  // images attached by button, paste or drop, the chat actions under `/`, the
  // model and thinking control, and Send, which becomes Stop while a reply
  // runs and nothing is typed. What is typed is kept per agent until it is
  // sent. While the connection is down, what is sent waits for it.

  interface Props {
    agent: string;
    /** A reply is under way. */
    replying: boolean;
    /** The agent's connection is down and coming back. */
    reconnecting: boolean;
    /** Messages waiting for the connection. */
    queued: number;
    /** A message, or a `/name text` line, to send. */
    onsend: (text: string, images?: ImageAttachment[]) => void;
    onstop: () => void;
  }

  let { agent, replying, reconnecting, queued, onsend, onstop }: Props = $props();

  /** The tallest the message box grows before it scrolls. */
  const MAX_FIELD_HEIGHT = 200;

  const uid = $props.id();
  const labelId = `${uid}-label`;
  const menuId = `${uid}-actions`;

  let text = $state(untrack(() => readDraft(agent)));
  let images = $state<ImageAttachment[]>(untrack(() => readDraftImages(agent)));
  let problem = $state<string | null>(null);
  let dragging = $state(false);
  let field = $state<HTMLTextAreaElement>();
  let filePicker = $state<HTMLInputElement>();

  $effect(() => {
    saveDraft(agent, text);
  });
  $effect(() => {
    saveDraftImages(agent, images);
  });

  // The message box grows with its text, up to a limit.
  $effect(() => {
    void text;
    const box = field;
    if (box === undefined) return;
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, MAX_FIELD_HEIGHT))}px`;
  });

  // ── The `/` menu ───────────────────────────────────────────────────

  let menuOpen = $state(false);
  let menuFromButton = $state(false);
  let menuQuery = $state("");
  let menuIndex = $state(0);
  const menuActions = $derived(matchActions(commandActions(actionRegistry.all), menuQuery));
  const showMenu = $derived(menuOpen && menuActions.length > 0);

  function openMenu(query: string, fromButton: boolean): void {
    menuOpen = true;
    menuFromButton = fromButton;
    menuQuery = query;
    menuIndex = 0;
  }

  // `/` as the first character, with no space yet, narrows the menu by what follows.
  function followTyping(): void {
    if (/^\/\S*$/.test(text)) openMenu(text.slice(1), false);
    else menuOpen = false;
  }

  function toggleMenu(): void {
    if (showMenu && menuFromButton) menuOpen = false;
    else openMenu("", true);
    field?.focus();
  }

  // A disabled action stays in the menu with its reason, and does nothing.
  function pick(action: AppAction): void {
    if (action.disabled !== undefined) return;
    menuOpen = false;
    if (action.takesText && action.command !== undefined) {
      text = `/${action.command} `;
      field?.focus();
      return;
    }
    text = "";
    void actionRegistry.run(action);
  }

  function menuKey(event: KeyboardEvent): boolean {
    const count = menuActions.length;
    switch (event.key) {
      case "ArrowDown":
        menuIndex = (menuIndex + 1) % count;
        return true;
      case "ArrowUp":
        menuIndex = (menuIndex - 1 + count) % count;
        return true;
      case "Tab": {
        const action = menuActions[menuIndex];
        if (action?.command === undefined) return false;
        // Filled in, to read over or add to before sending.
        text = `/${action.command}${action.takesText === true ? " " : ""}`;
        menuOpen = false;
        return true;
      }
      case "Enter": {
        const action = menuActions[menuIndex];
        if (action !== undefined) pick(action);
        return true;
      }
      case "Escape":
        // Claimed here, so Esc closes the menu rather than stopping the reply.
        menuOpen = false;
        return true;
      default:
        return false;
    }
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.isComposing) return;
    if (showMenu && menuKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send();
    }
  }

  // ── Images ─────────────────────────────────────────────────────────

  async function attach(files: Iterable<File>): Promise<void> {
    const read = await readImages(files);
    images = [...images, ...read.images];
    problem = read.problem;
  }

  function onpaste(event: ClipboardEvent): void {
    const pasted = [...(event.clipboardData?.files ?? [])];
    if (pasted.length === 0) return;
    event.preventDefault();
    void attach(pasted);
  }

  const carriesFiles = (event: DragEvent): boolean =>
    event.dataTransfer?.types.includes("Files") ?? false;

  function ondragover(event: DragEvent): void {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    dragging = true;
  }

  function ondragleave(event: DragEvent & { currentTarget: HTMLElement }): void {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) dragging = false;
  }

  function ondrop(event: DragEvent): void {
    if (!carriesFiles(event)) return;
    event.preventDefault();
    dragging = false;
    void attach(event.dataTransfer?.files ?? []);
  }

  // ── Sending ────────────────────────────────────────────────────────

  const empty = $derived(text.trim() === "" && images.length === 0);
  // With nothing typed while a reply runs, Send is Stop; typing brings Send
  // back, for a message that steers the reply.
  const showStop = $derived(replying && empty);

  function send(): void {
    if (empty) return;
    onsend(text.trim(), images.length > 0 ? images : undefined);
    text = "";
    images = [];
    problem = null;
    menuOpen = false;
  }

  // A press anywhere else closes the menu.
  function onfocusout(event: FocusEvent & { currentTarget: HTMLElement }): void {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) menuOpen = false;
  }

  const queuedLine = $derived(
    queued === 0
      ? "Reconnecting — messages you send now will go out once back online."
      : `Reconnecting — ${queued === 1 ? "1 message" : `${String(queued)} messages`} will send once back online.`,
  );
</script>

<form
  class="composer"
  data-dragging={dragging || undefined}
  onsubmit={(event) => {
    event.preventDefault();
    send();
  }}
  {ondragover}
  {ondragleave}
  {ondrop}
  {onfocusout}
>
  {#if showMenu}
    <SlashMenu
      id={menuId}
      actions={menuActions}
      active={menuIndex}
      onpick={pick}
      onhover={(index) => (menuIndex = index)}
    />
  {/if}
  {#if reconnecting}
    <p class="composer-line" role="status"><Icon name="wifi-off" size={14} />{queuedLine}</p>
  {/if}
  {#if problem !== null}
    <p class="composer-line composer-problem" role="alert">{problem}</p>
  {/if}
  {#if images.length > 0}
    <ul class="composer-images" aria-label="Attached images">
      {#each images as image, index (image)}
        <li class="composer-image">
          <img src="data:{image.media_type};base64,{image.data}" alt="Image {index + 1}" />
          <IconButton
            icon="close"
            size="sm"
            variant="secondary"
            label="Remove image {index + 1}"
            onclick={() => (images = images.filter((other) => other !== image))}
          />
        </li>
      {/each}
    </ul>
  {/if}
  <VisuallyHidden id={labelId}>Message {hub.shownName(agent)}</VisuallyHidden>
  <div
    class="composer-field"
    role="combobox"
    aria-expanded={showMenu}
    aria-controls={menuId}
    aria-labelledby={labelId}
  >
    <textarea
      bind:this={field}
      bind:value={text}
      rows="1"
      placeholder="Message {hub.shownName(agent)}"
      aria-labelledby={labelId}
      aria-autocomplete="list"
      aria-controls={showMenu ? menuId : undefined}
      aria-activedescendant={showMenu ? `${menuId}-${String(menuIndex)}` : undefined}
      {onkeydown}
      oninput={followTyping}
      {onpaste}
    ></textarea>
  </div>
  <div class="composer-bar">
    <IconButton icon="paperclip" label="Attach images" onclick={() => filePicker?.click()} />
    <IconButton
      icon="slash"
      label="Chat actions"
      aria-expanded={showMenu && menuFromButton}
      aria-controls={menuId}
      onclick={toggleMenu}
    />
    <ModelControl {agent} />
    <span class="composer-send">
      {#if showStop}
        <IconButton icon="stop" variant="secondary" label="Stop reply" onclick={onstop} />
      {:else}
        <IconButton icon="send" variant="primary" label="Send" type="submit" disabled={empty} />
      {/if}
    </span>
  </div>
  <input
    bind:this={filePicker}
    type="file"
    accept={IMAGE_TYPES.join(",")}
    multiple
    hidden
    onchange={(event) => {
      const input = event.currentTarget;
      void attach([...(input.files ?? [])]);
      // The same file can be picked again.
      input.value = "";
    }}
  />
</form>

<style>
  .composer {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    max-width: var(--layout-reading-width);
    margin: 0 auto;
    padding: var(--space-10) var(--space-10) var(--space-8) var(--space-14);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
    box-shadow: inset 0 0 0 1px var(--color-line-soft);
    transition: box-shadow var(--duration-fast) var(--ease-out);

    &:focus-within {
      box-shadow:
        inset 0 0 0 1px var(--color-vein-line),
        0 0 0 3px var(--color-vein-faint);
    }

    &[data-dragging] {
      box-shadow:
        inset 0 0 0 2px var(--color-vein),
        0 0 0 3px var(--color-vein-faint);
    }
  }

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

  .composer-images {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
    list-style: none;
  }

  .composer-image {
    position: relative;

    & img {
      display: block;
      width: 56px;
      height: 56px;
      border-radius: var(--corner-md);
      object-fit: cover;
    }

    & :global(.ui-icon-button) {
      position: absolute;
      top: calc(-1 * var(--space-6));
      right: calc(-1 * var(--space-6));
    }
  }

  .composer-field textarea {
    display: block;
    width: 100%;
    min-height: 24px;
    max-height: 200px;
    padding: var(--space-2) 0;
    resize: none;
    border: 0;
    background: transparent;
    font-size: var(--font-size-message);
    line-height: var(--line-height-ui);

    /* The composer's own ring shows focus. */
    &:focus-visible {
      outline: none;
    }
  }

  .composer-bar {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    min-width: 0;
  }

  .composer-send {
    display: flex;
    flex: none;
    margin-left: auto;

    & :global(.ui-icon-button[data-variant="primary"]:disabled) {
      background: var(--color-stone-3);
      color: var(--color-text-3);
      opacity: 1;
    }
  }

  @media (max-width: 760px) {
    .composer {
      padding: var(--space-8) var(--space-8) var(--space-6) var(--space-12);
    }

    .composer-field textarea {
      font-size: var(--font-size-field-phone);
    }
  }
</style>
