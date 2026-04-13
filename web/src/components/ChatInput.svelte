<script lang="ts">
  import { filterCommands, COMMAND_REGISTRY } from "../lib/commands";
  import type { ImageAttachment } from "../lib/types";
  import { clickOutside } from "../lib/actions/clickOutside";
  import { Icon } from "../lib/icons";
  import CommandMenu from "./CommandMenu.svelte";
  import ModelSelector from "./ModelSelector.svelte";
  import ThinkingSelector from "./ThinkingSelector.svelte";

  const MAX_IMAGE_BYTES = 5 * 1024 * 1024; // 5 MB
  const MAX_IMAGES = 5;
  const ACCEPTED_TYPES = ["image/jpeg", "image/png", "image/gif", "image/webp"];

  let {
    onSend,
    disabled = false,
  }: { onSend: (text: string, images?: ImageAttachment[]) => void; disabled?: boolean } = $props();
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

  // Autocomplete state
  let showMenu = $state(false);
  let menuQuery = $state("");
  let menuIndex = $state(0);
  let menuFromButton = $state(false);
  let containerEl: HTMLDivElement | undefined = $state();

  let filtered = $derived.by(() => {
    if (menuFromButton && !menuQuery) return COMMAND_REGISTRY;
    return filterCommands(menuQuery);
  });

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
            const cmd = filtered[menuIndex];
            if (cmd) {
              // Complete inline so the user can review/edit before sending.
              value = cmd.name + (cmd.hasArgs ? " " : "");
              showMenu = false;
              textarea?.focus();
            }
          }
          return;
        case "Enter":
          e.preventDefault();
          {
            const cmd = filtered[menuIndex];
            if (cmd) handleCommandSelect(cmd.name);
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
  }

  function handleCommandSelect(name: string) {
    const cmd = COMMAND_REGISTRY.find((c) => c.name === name);
    if (cmd?.hasArgs) {
      value = name + " ";
      showMenu = false;
      textarea?.focus();
    } else {
      showMenu = false;
      value = "";
      onSend(name);
    }
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
      if (pendingImages.length >= MAX_IMAGES) {
        showRejection(`Maximum ${MAX_IMAGES} images`);
        break;
      }
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
      <CommandMenu commands={filtered} selectedIndex={menuIndex} onSelect={handleCommandSelect} />
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
          {disabled}
          onkeydown={handleKeydown}
          oninput={handleInput}
          onpaste={handlePaste}
        ></textarea>
        <button
          class="send-btn"
          onclick={submit}
          disabled={disabled || (!value.trim() && !pendingImages.length)}>Send</button
        >
      </div>
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
            title="Commands"
            aria-label="Commands"
          >
            /
          </button>
        </div>
        <div class="chat-toolbar-right">
          <ModelSelector {disabled} />
          <ThinkingSelector {disabled} />
          <button
            class="send-btn-toolbar"
            onclick={submit}
            disabled={disabled || (!value.trim() && !pendingImages.length)}>Send</button
          >
        </div>
      </div>
    </div>
  </div>
</div>
