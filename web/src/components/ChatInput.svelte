<script lang="ts">
  import {
    actionRegistry,
    commandActions,
    matchActions,
    type AppAction,
  } from "../lib/action-registry.svelte";
  import type { ImageAttachment } from "../lib/types";
  import { clickOutside } from "../lib/actions/clickOutside";
  import { Icon } from "../lib/icons";
  import SlashMenu from "./SlashMenu.svelte";
  import ModelSelector from "./ModelSelector.svelte";
  import ThinkingSelector from "./ThinkingSelector.svelte";

  // Model API limits, not residuum's own: 5 MB and these MIME types per image.
  // There is no limit on how many images a message can carry.
  const MAX_IMAGE_BYTES = 5 * 1024 * 1024; // 5 MB
  const ACCEPTED_TYPES = ["image/jpeg", "image/png", "image/gif", "image/webp"];

  let {
    onSend,
    onStop,
    isProcessing = false,
    disabled = false,
    reconnecting = false,
    pendingCount = 0,
  }: {
    onSend: (text: string, images?: ImageAttachment[]) => void;
    onStop: () => void;
    isProcessing?: boolean;
    /** Blocks drafting and sending outright, for a reason other than the
     * connection (there is currently no such caller). Reconnecting never
     * sets this — see `reconnecting` below. */
    disabled?: boolean;
    /** The connection is down or coming back up. Drafting and sending stay
     * enabled; a sent message queues at the transport layer and is shown
     * as pending until it actually goes out. */
    reconnecting?: boolean;
    /** How many of the user's own messages are queued waiting to send. */
    pendingCount?: number;
  } = $props();
  let value = $state("");
  let textarea: HTMLTextAreaElement | undefined = $state();
  let fileInput: HTMLInputElement | undefined;
  let pendingImages = $state<ImageAttachment[]>([]);
  let rejectionMsg = $state("");
  let rejectionTimer: ReturnType<typeof setTimeout> | undefined;
  let dragging = $state(false);

  function showRejection(msg: string) {
    rejectionMsg = msg;
    clearTimeout(rejectionTimer);
    rejectionTimer = setTimeout(() => {
      rejectionMsg = "";
    }, 3000);
  }

  // The `/` menu: the registry's chat actions, narrowed by what follows the `/`.
  const menuId = $props.id();
  let showMenu = $state(false);
  let menuQuery = $state("");
  let menuIndex = $state(0);
  let menuFromButton = $state(false);
  let containerEl: HTMLDivElement | undefined = $state();

  let filtered = $derived(matchActions(commandActions(actionRegistry.all), menuQuery));

  // While a turn is running with nothing typed, the send button becomes a
  // stop button — start typing a steering message and send comes back.
  let showStop = $derived(isProcessing && !value.trim() && !pendingImages.length);

  function autoResize() {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 160)}px`;
  }

  function handleInput() {
    autoResize();
    // Trigger autocomplete when typing starts with / and has no space yet
    if (value.startsWith("/") && !value.includes(" ")) {
      showMenu = true;
      menuQuery = value.slice(1);
      menuFromButton = false;
      menuIndex = 0;
    } else {
      showMenu = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (showMenu && filtered.length > 0) {
      switch (e.key) {
        case "ArrowUp":
          e.preventDefault();
          menuIndex = menuIndex > 0 ? menuIndex - 1 : filtered.length - 1;
          return;
        case "ArrowDown":
          e.preventDefault();
          menuIndex = menuIndex < filtered.length - 1 ? menuIndex + 1 : 0;
          return;
        case "Tab":
          e.preventDefault();
          {
            const action = filtered[menuIndex];
            if (action?.command !== undefined) {
              // Complete inline so the user can review/edit before sending.
              value = `/${action.command}${action.takesText ? " " : ""}`;
              showMenu = false;
              textarea?.focus();
            }
          }
          return;
        case "Enter":
          e.preventDefault();
          {
            const action = filtered[menuIndex];
            if (action) handleCommandSelect(action);
          }
          return;
        case "Escape":
          e.preventDefault();
          showMenu = false;
          return;
      }
    }

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }

    // Scoped to the composer having focus (rather than a global window
    // listener) so it never races the app's several other Escape handlers
    // — modals, drawers, the command menu above, the shortcuts overlay —
    // each of which owns Escape only while its own overlay is open, with
    // no shared priority order between them. A user about to stop a
    // running turn is naturally focused here already, or one click away.
    if (e.key === "Escape" && isProcessing) {
      e.preventDefault();
      onStop();
    }
  }

  // A disabled action stays in the menu with its reason, and does nothing.
  function handleCommandSelect(action: AppAction) {
    if (action.disabled !== undefined) return;
    showMenu = false;
    if (action.takesText && action.command !== undefined) {
      value = `/${action.command} `;
      textarea?.focus();
      return;
    }
    value = "";
    void actionRegistry.run(action);
  }

  function toggleCommandMenu() {
    if (disabled) return;
    if (showMenu && menuFromButton) {
      showMenu = false;
    } else {
      showMenu = true;
      menuFromButton = true;
      menuQuery = "";
      menuIndex = 0;
    }
  }

  function readFileAsBase64(file: File): Promise<ImageAttachment> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result as string;
        // Strip data URL prefix: "data:image/png;base64,..."
        const base64 = result.split(",")[1] ?? "";
        resolve({ media_type: file.type, data: base64 });
      };
      reader.onerror = () => reject(reader.error ?? new Error("FileReader failed"));
      reader.readAsDataURL(file);
    });
  }

  async function handleFiles(files: FileList | File[]) {
    for (const file of files) {
      if (!ACCEPTED_TYPES.includes(file.type)) {
        showRejection("Unsupported file type");
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        showRejection("File too large (5MB max)");
        continue;
      }
      const img = await readFileAsBase64(file);
      pendingImages = [...pendingImages, img];
    }
  }

  function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (input.files?.length) void handleFiles(input.files);
    input.value = ""; // allow re-selecting the same file
  }

  function removeImage(index: number) {
    pendingImages = pendingImages.filter((_, i) => i !== index);
  }

  function handlePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (const item of items) {
      if (item.kind === "file" && ACCEPTED_TYPES.includes(item.type)) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length) {
      e.preventDefault();
      void handleFiles(imageFiles);
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    dragging = true;
  }

  function handleDragLeave(e: DragEvent) {
    if (containerEl && !containerEl.contains(e.relatedTarget as Node)) {
      dragging = false;
    }
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    if (e.dataTransfer?.files.length) void handleFiles(e.dataTransfer.files);
  }

  function submit() {
    const text = value.trim();
    if ((!text && !pendingImages.length) || disabled) return;
    const images = pendingImages.length ? pendingImages : undefined;
    onSend(text, images);
    value = "";
    pendingImages = [];
    showMenu = false;
    if (textarea) textarea.style.height = "auto";
  }
</script>

<div
  class="chat-input-area"
  bind:this={containerEl}
  use:clickOutside={{
    onOutside: () => {
      showMenu = false;
    },
  }}
>
  <div class="chat-input-container">
    {#if showMenu && filtered.length > 0}
      <SlashMenu
        id={menuId}
        actions={filtered}
        active={menuIndex}
        onpick={handleCommandSelect}
        onhover={(index) => (menuIndex = index)}
      />
    {/if}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="chat-input-wrap"
      class:dragging
      ondragover={handleDragOver}
      ondragleave={handleDragLeave}
      ondrop={handleDrop}
    >
      {#if rejectionMsg}
        <div class="image-rejection-msg">{rejectionMsg}</div>
      {/if}
      {#if pendingImages.length > 0}
        <div class="image-preview-strip">
          {#each pendingImages as img, i (i)}
            <div class="image-preview-item">
              <img
                src="data:{img.media_type};base64,{img.data}"
                alt="attachment"
                class="image-preview-thumb"
              />
              <button class="image-preview-remove" onclick={() => removeImage(i)} title="Remove"
                >&times;</button
              >
            </div>
          {/each}
        </div>
      {/if}
      <div class="chat-input-row">
        <textarea
          bind:this={textarea}
          bind:value
          class="chat-input"
          placeholder="Send a message..."
          rows="1"
          aria-controls={showMenu && filtered.length > 0 ? menuId : undefined}
          aria-activedescendant={showMenu && filtered.length > 0
            ? `${menuId}-${String(menuIndex)}`
            : undefined}
          {disabled}
          onkeydown={handleKeydown}
          oninput={handleInput}
          onpaste={handlePaste}
        ></textarea>
        {#if showStop}
          <button class="stop-btn" onclick={onStop} aria-label="Stop the agent">
            <Icon name="stop" size={14} />
          </button>
        {:else}
          <button
            class="send-btn"
            onclick={submit}
            disabled={disabled || (!value.trim() && !pendingImages.length)}>Send</button
          >
        {/if}
      </div>
      {#if reconnecting || pendingCount > 0}
        <p class="chat-input-status">
          {#if pendingCount > 0}
            Reconnecting — {pendingCount === 1 ? "1 message" : `${pendingCount} messages`} will send once
            back online.
          {:else}
            Reconnecting — messages you send now will go out once back online.
          {/if}
        </p>
      {/if}
      <input
        bind:this={fileInput}
        type="file"
        accept="image/jpeg,image/png,image/gif,image/webp"
        multiple
        class="hidden-file-input"
        onchange={handleFileSelect}
      />
      <div class="chat-toolbar">
        <div class="chat-toolbar-left">
          <button
            class="attach-btn"
            onclick={() => fileInput?.click()}
            {disabled}
            title="Attach image"
            aria-label="Attach image"
          >
            <Icon name="paperclip" size={16} />
          </button>
          <button
            class="cmd-menu-btn"
            onclick={toggleCommandMenu}
            {disabled}
            title="Chat actions"
            aria-label="Chat actions"
          >
            /
          </button>
        </div>
        <div class="chat-toolbar-right">
          <ModelSelector {disabled} />
          <ThinkingSelector {disabled} />
          {#if showStop}
            <button class="stop-btn-toolbar" onclick={onStop} aria-label="Stop the agent">
              <Icon name="stop" size={12} />
            </button>
          {:else}
            <button
              class="send-btn-toolbar"
              onclick={submit}
              disabled={disabled || (!value.trim() && !pendingImages.length)}>Send</button
            >
          {/if}
        </div>
      </div>
    </div>
  </div>
</div>
