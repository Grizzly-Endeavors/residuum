<script lang="ts">
  import { tick } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { MAX_UPLOAD_BYTES, UPLOAD_LIMIT_LABEL } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import type { RoomDetail, RoomSeat } from "../../lib/hub-types";
  import { roomsStore } from "../../lib/rooms.svelte";
  import { joinNames } from "../../lib/words";
  import {
    AttachmentChip,
    Badge,
    Button,
    IconButton,
    VisuallyHidden,
    type AttachmentState,
  } from "../../lib/ui";

  interface Props {
    room: RoomDetail;
    turnActive: boolean;
    reopening?: boolean;
    onreopen?: () => void;
  }

  let { room, turnActive, reopening = false, onreopen }: Props = $props();

  interface MentionOption {
    id: string;
    token: string;
    label: string;
    hint: string | null;
    kind: "everyone" | RoomSeat["kind"];
    instance: string | null;
  }

  interface StagedRoomFile {
    key: string;
    file: File;
    filename: string;
    size: number;
    contentType: string;
    status: "uploading" | "uploaded" | "failed";
    uploadId: string | null;
    error: string | null;
    controller: AbortController | null;
  }

  const MAX_FIELD_HEIGHT = 200;
  const uid = $props.id();
  const labelId = `${uid}-label`;
  const menuId = `${uid}-mentions`;
  const hintId = `${uid}-hint`;
  const touch = new MediaQuery("(pointer: coarse)");

  let text = $state("");
  let sending = $state(false);
  let sendError = $state<string | null>(null);
  let sendHeld = $state(false);
  let field = $state<HTMLTextAreaElement>();
  let filePicker = $state<HTMLInputElement>();

  // ── Staged attachments ─────────────────────────────────────────────

  let fileCounter = 0;
  let stagedFiles = $state<StagedRoomFile[]>([]);
  let fileNotice = $state<string | null>(null);

  const uploading = $derived(stagedFiles.some((f) => f.status === "uploading"));
  const failedFiles = $derived(stagedFiles.filter((f) => f.status === "failed"));

  const fileProblem = $derived.by<string | null>(() => {
    if (failedFiles.length > 0) {
      const one = failedFiles.length === 1;
      return `${joinNames(failedFiles.map((f) => f.filename))} didn't upload. Try ${one ? "it" : "them"} again or remove ${one ? "it" : "them"}, then send.`;
    }
    if (uploading) {
      const names = joinNames(
        stagedFiles.filter((f) => f.status === "uploading").map((f) => f.filename),
      );
      return `Wait for ${names} to finish uploading, then send.`;
    }
    return null;
  });

  $effect(() => {
    if (fileProblem === null) sendHeld = false;
  });

  function startUpload(entry: StagedRoomFile): void {
    const controller = new AbortController();
    entry.status = "uploading";
    entry.error = null;
    entry.controller = controller;
    void roomsStore
      .uploadAttachment(entry.file, entry.filename, { signal: controller.signal })
      .then((uploaded) => {
        if (entry.controller !== controller) return;
        entry.uploadId = uploaded.id;
        entry.status = "uploaded";
      })
      .catch((err: unknown) => {
        if (entry.controller !== controller) return;
        entry.error = userErrorMessage(err, { action: `Couldn't upload ${entry.filename}.` });
        entry.status = "failed";
      });
  }

  function attachFiles(files: Iterable<File>): void {
    const refused: string[] = [];
    const added: StagedRoomFile[] = [];
    for (const file of files) {
      if (file.size > MAX_UPLOAD_BYTES) {
        refused.push(file.name);
        continue;
      }
      const entry = $state<StagedRoomFile>({
        key: `room-file-${String(++fileCounter)}`,
        file,
        filename: file.name,
        size: file.size,
        contentType: file.type,
        status: "uploading",
        uploadId: null,
        error: null,
        controller: null,
      });
      startUpload(entry);
      added.push(entry);
    }
    if (refused.length === 0) {
      fileNotice = null;
    } else {
      const one = refused.length === 1;
      fileNotice = `${joinNames(refused)} ${one ? "is" : "are"} larger than the ${UPLOAD_LIMIT_LABEL} upload limit, so ${one ? "it wasn't" : "they weren't"} attached.`;
    }
    if (added.length > 0) stagedFiles = [...stagedFiles, ...added];
  }

  function removeFile(key: string): void {
    const target = stagedFiles.find((f) => f.key === key);
    if (target === undefined) return;
    target.controller?.abort();
    target.controller = null;
    stagedFiles = stagedFiles.filter((f) => f.key !== key);
  }

  function retryFile(key: string): void {
    const target = stagedFiles.find((f) => f.key === key);
    if (target?.status !== "failed") return;
    startUpload(target);
  }

  function attachmentState(status: StagedRoomFile["status"]): AttachmentState {
    switch (status) {
      case "uploading":
        return "pending";
      case "failed":
        return "failed";
      case "uploaded":
        return "ready";
    }
  }

  function onpaste(event: ClipboardEvent): void {
    if (archived) return;
    const pasted = [...(event.clipboardData?.files ?? [])];
    if (pasted.length === 0) return;
    event.preventDefault();
    attachFiles(pasted);
  }

  // ── Autoresize ─────────────────────────────────────────────────────

  $effect(() => {
    void text;
    const box = field;
    if (box === undefined) return;
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, MAX_FIELD_HEIGHT))}px`;
  });

  // ── @ mention autocomplete ─────────────────────────────────────────

  const archived = $derived(room.status === "archived");

  function leafSlugOf(seatId: string): string {
    const stripped = seatId.replace(/^(?:agent:|a2a:)/, "");
    const slash = stripped.indexOf("/");
    return slash >= 0 ? stripped.slice(slash + 1) : stripped;
  }

  function fullSlugOf(seatId: string): string {
    return seatId.replace(/^(?:agent:|a2a:)/, "");
  }

  const allMentionOptions = $derived.by<MentionOption[]>(() => {
    const leafCounts: Record<string, number> = {};
    for (const seat of room.roster) {
      const leaf = leafSlugOf(seat.seat_id).toLowerCase();
      leafCounts[leaf] = (leafCounts[leaf] ?? 0) + 1;
    }
    const seats: MentionOption[] = room.roster.map((seat) => {
      const leaf = leafSlugOf(seat.seat_id);
      const token = (leafCounts[leaf.toLowerCase()] ?? 0) > 1 ? fullSlugOf(seat.seat_id) : leaf;
      return {
        id: seat.seat_id,
        token,
        label: seat.display_name,
        hint: seat.role,
        kind: seat.kind,
        instance: seat.instance,
      };
    });
    return [
      {
        id: "everyone",
        token: "everyone",
        label: "everyone",
        hint: "Queue every active participant once",
        kind: "everyone",
        instance: null,
      },
      ...seats,
    ];
  });

  let mentionOpen = $state(false);
  let mentionQuery = $state("");
  let mentionStart = $state(0);
  let mentionEnd = $state(0);
  let mentionIndex = $state(0);

  const filteredMentions = $derived.by(() => {
    const q = mentionQuery.trim().toLowerCase();
    if (q === "") return allMentionOptions;
    return allMentionOptions.filter(
      (opt) =>
        opt.token.toLowerCase().includes(q) ||
        opt.label.toLowerCase().includes(q) ||
        (opt.hint?.toLowerCase().includes(q) ?? false),
    );
  });

  const showMentionMenu = $derived(!archived && mentionOpen && filteredMentions.length > 0);

  function detectMention(): void {
    if (archived || field === undefined) {
      mentionOpen = false;
      return;
    }
    const cursor = field.selectionStart ?? text.length;
    const before = text.slice(0, cursor);
    const match = /(?:^|[\s(])@([^\s@]*)$/.exec(before);
    if (match === null) {
      mentionOpen = false;
      return;
    }
    const query = match[1] ?? "";
    const atPosition = cursor - query.length - 1;
    const queryChanged = query !== mentionQuery || !mentionOpen;
    mentionStart = atPosition;
    mentionEnd = cursor;
    mentionQuery = query;
    mentionOpen = true;
    if (queryChanged) mentionIndex = 0;
  }

  async function pickMention(option: MentionOption): Promise<void> {
    const prefix = text.slice(0, mentionStart);
    const suffix = text.slice(mentionEnd);
    const inserted = `@${option.token} `;
    const nextCursor = prefix.length + inserted.length;
    text = `${prefix}${inserted}${suffix}`;
    mentionOpen = false;
    mentionQuery = "";
    mentionIndex = 0;
    await tick();
    if (field !== undefined) {
      field.focus();
      field.setSelectionRange(nextCursor, nextCursor);
    }
  }

  async function insertMentionTrigger(): Promise<void> {
    if (archived || field === undefined) return;
    const cursor = field.selectionStart ?? text.length;
    const prefix = text.slice(0, cursor);
    const suffix = text.slice(field.selectionEnd ?? cursor);
    const needsSpace = prefix.length > 0 && !/[\s(]$/.test(prefix);
    const inserted = needsSpace ? " @" : "@";
    const nextCursor = prefix.length + inserted.length;
    text = `${prefix}${inserted}${suffix}`;
    await tick();
    if (field !== undefined) {
      field.focus();
      field.setSelectionRange(nextCursor, nextCursor);
      detectMention();
    }
  }

  function mentionKey(event: KeyboardEvent): boolean {
    const count = filteredMentions.length;
    if (count === 0) return false;
    switch (event.key) {
      case "ArrowDown":
        mentionIndex = (mentionIndex + 1) % count;
        return true;
      case "ArrowUp":
        mentionIndex = (mentionIndex - 1 + count) % count;
        return true;
      case "Tab":
      case "Enter": {
        const option = filteredMentions[mentionIndex];
        if (option !== undefined) void pickMention(option);
        return true;
      }
      case "Escape":
        mentionOpen = false;
        return true;
      default:
        return false;
    }
  }

  // ── Sending ────────────────────────────────────────────────────────

  const empty = $derived(text.trim() === "" && stagedFiles.length === 0);
  const heldReason = $derived(sendHeld ? fileProblem : null);

  async function send(): Promise<void> {
    if (archived || turnActive || sending || empty) return;
    if (fileProblem !== null) {
      sendHeld = true;
      return;
    }
    const uploadIds = stagedFiles.map((f) => f.uploadId).filter((id): id is string => id !== null);
    sending = true;
    sendError = null;
    const posted = await roomsStore.postMessage(room.id, text.trim(), uploadIds);
    sending = false;
    if (posted === null) {
      sendError = roomsStore.error ?? "Couldn't send the message.";
      return;
    }
    text = "";
    stagedFiles = [];
    fileNotice = null;
    sendHeld = false;
    mentionOpen = false;
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.isComposing) return;
    if (showMentionMenu && mentionKey(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !touch.current) {
      event.preventDefault();
      void send();
    }
  }

  function onsubmit(event: SubmitEvent): void {
    event.preventDefault();
    void send();
  }

  function onfocusout(event: FocusEvent & { currentTarget: HTMLElement }): void {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    mentionOpen = false;
  }

  function optionId(index: number): string {
    return `${menuId}-${String(index)}`;
  }
</script>

<div class="room-composer-wrap">
  {#if archived}
    <div class="room-archived-bar" role="status">
      <span class="room-archived-copy">This meeting room is archived and read-only.</span>
      {#if onreopen !== undefined}
        <Button size="sm" variant="secondary" icon="restore" loading={reopening} onclick={onreopen}>
          Reopen room
        </Button>
      {/if}
    </div>
  {/if}

  <form
    class="composer"
    data-disabled={archived || undefined}
    aria-labelledby={labelId}
    {onsubmit}
    {onfocusout}
  >
    {#if showMentionMenu}
      <div id={menuId} class="mention-menu" role="listbox" aria-label="Mention participant">
        {#each filteredMentions as option, index (option.id)}
          {@const active = index === mentionIndex}
          <button
            type="button"
            id={optionId(index)}
            class="mention-option"
            role="option"
            tabindex="-1"
            aria-selected={active}
            onpointerdown={(event) => {
              event.preventDefault();
            }}
            onpointermove={() => {
              if (!active) mentionIndex = index;
            }}
            onclick={() => void pickMention(option)}
          >
            <span class="mention-token">@{option.token}</span>
            {#if option.kind !== "everyone"}
              <span class="mention-label"> {option.label}</span>
            {/if}
            {#if option.kind === "external_a2a"}
              <Badge tone="danger">EXT A2A</Badge>
            {:else if option.kind === "sibling_a2a" && option.instance !== null}
              <Badge tone="accent">{option.instance}</Badge>
            {/if}
            {#if option.hint !== null && option.hint.trim() !== ""}
              <span class="mention-hint">{option.hint}</span>
            {/if}
          </button>
        {/each}
      </div>
    {/if}

    {#if fileNotice !== null}
      <p class="composer-line" role="status">
        <span>{fileNotice}</span>
        <Button variant="quiet" size="sm" onclick={() => (fileNotice = null)}>Dismiss</Button>
      </p>
    {/if}

    {#if stagedFiles.length > 0}
      <ul class="staged-files" aria-label="Attached files">
        {#each stagedFiles as attachment (attachment.key)}
          <li>
            <AttachmentChip
              filename={attachment.filename}
              size={attachment.size}
              contentType={attachment.contentType || undefined}
              state={attachmentState(attachment.status)}
              failure={attachment.error ?? undefined}
              onretry={() => retryFile(attachment.key)}
              onremove={() => removeFile(attachment.key)}
            />
          </li>
        {/each}
      </ul>
    {/if}

    {#if heldReason !== null}
      <p class="composer-line composer-problem" role="alert">{heldReason}</p>
    {:else if sendError !== null}
      <p class="composer-line composer-problem" role="alert">{sendError}</p>
    {/if}

    <VisuallyHidden id={labelId}>Message {room.name}</VisuallyHidden>
    <div class="composer-field">
      <textarea
        bind:this={field}
        bind:value={text}
        rows="1"
        disabled={archived}
        placeholder={archived
          ? "This room is archived. Reopen it to send messages."
          : `Message ${room.name} (type @ to mention)`}
        role={archived ? undefined : "combobox"}
        aria-labelledby={labelId}
        aria-describedby={turnActive ? hintId : undefined}
        aria-autocomplete={archived ? undefined : "list"}
        aria-expanded={archived ? undefined : showMentionMenu}
        aria-controls={showMentionMenu ? menuId : undefined}
        aria-activedescendant={showMentionMenu ? optionId(mentionIndex) : undefined}
        enterkeyhint={touch.current ? "enter" : undefined}
        {onkeydown}
        oninput={detectMention}
        {onpaste}
      ></textarea>
    </div>

    <div class="composer-bar">
      <IconButton
        icon="paperclip"
        label="Attach files"
        disabled={archived}
        onclick={() => filePicker?.click()}
      />
      <Button
        size="sm"
        variant="quiet"
        disabled={archived}
        aria-label="Mention participant"
        onclick={() => void insertMentionTrigger()}
      >
        @
      </Button>
      {#if turnActive}
        <span id={hintId} class="composer-hint">Wait for the current reply or press Stop</span>
      {/if}
      <span class="composer-send">
        <IconButton
          icon="send"
          variant="primary"
          label="Send"
          type="submit"
          loading={sending}
          disabled={archived || turnActive || empty}
          onmousedown={(event) => event.preventDefault()}
        />
      </span>
    </div>

    <input
      bind:this={filePicker}
      type="file"
      multiple
      hidden
      onchange={(event) => {
        const input = event.currentTarget;
        attachFiles([...(input.files ?? [])]);
        input.value = "";
      }}
    />
  </form>
</div>

<style>
  .room-composer-wrap {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    width: 100%;
    max-width: var(--layout-reading-width);
    margin: 0 auto;
  }

  .room-archived-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-10);
    padding: var(--space-8) var(--space-14);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
  }

  .room-archived-copy {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .composer {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding: var(--space-10) var(--space-10) var(--space-8) var(--space-14);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
    box-shadow: inset 0 0 0 1px var(--color-control-border);
    transition: box-shadow var(--duration-fast) var(--ease-out);

    &:focus-within:not([data-disabled]) {
      box-shadow:
        inset 0 0 0 2px var(--color-vein),
        0 0 0 3px var(--color-vein-faint);
    }

    &[data-disabled] {
      opacity: 0.65;
    }
  }

  .mention-menu {
    position: absolute;
    right: 0;
    bottom: calc(100% + var(--space-6));
    left: 0;
    z-index: var(--z-sticky);
    max-height: min(280px, 45vh);
    padding: var(--space-6);
    overflow-y: auto;
    border-radius: var(--corner-lg);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
  }

  .mention-option {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    min-height: 36px;
    padding: 0 var(--space-10);
    border: 0;
    border-radius: var(--corner-md);
    background: transparent;
    color: var(--color-text-2);
    font-family: var(--font-ui);
    font-size: var(--font-size-sm);
    text-align: start;
    cursor: pointer;

    &[aria-selected="true"] {
      background: var(--color-vein-tint);
      color: var(--color-text);

      &::before {
        content: "";
        position: absolute;
        top: var(--space-8);
        bottom: var(--space-8);
        left: 0;
        width: var(--space-2);
        border-radius: var(--corner-pill);
        background: var(--color-vein-bright);
      }
    }
  }

  .mention-token {
    flex: none;
    color: var(--color-vein-bright);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-semibold);
  }

  .mention-label {
    flex: none;
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .mention-hint {
    min-width: 0;
    margin-left: auto;
    overflow: hidden;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .staged-files {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }

  .composer-line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .composer-problem {
    color: var(--color-err-text);
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
    color: var(--color-text);
    font-size: var(--font-size-message);
    --leading: var(--line-height-ui);

    &:focus-visible {
      outline: none;
    }

    &:disabled {
      cursor: not-allowed;
    }
  }

  .composer-bar {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    min-width: 0;
  }

  .composer-hint {
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .composer-send {
    display: flex;
    align-items: center;
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

    .mention-option {
      min-height: var(--layout-touch-target);
    }
  }
</style>
