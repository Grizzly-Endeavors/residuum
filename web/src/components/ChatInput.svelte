<script lang="ts">
  import { filterCommands, COMMAND_REGISTRY } from "../lib/commands";
  import type { ImageAttachment } from "../lib/types";
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
  let fileInput: HTMLInputElement | undefined = $state();
  let pendingImages = $state<ImageAttachment[]>([]);
  let rejectionMsg = $state("");
  let rejectionTimer: ReturnType<typeof setTimeout> | undefined;

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

  $effect(() => {
    // trigger resize when value changes
    value;
    autoResize();
  });

  // Click-outside to dismiss menu
  $effect(() => {
    if (!showMenu) return;
    function handleClick(e: MouseEvent) {
      if (containerEl && !containerEl.contains(e.target as Node)) {
        showMenu = false;
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  });
</script>

<div class="chat-input-area" bind:this={containerEl}>
  <div class="chat-input-container">
    {#if showMenu && filtered.length > 0}
      <CommandMenu commands={filtered} selectedIndex={menuIndex} onSelect={handleCommandSelect} />
    {/if}
    <div class="chat-input-wrap">
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
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              xmlns="http://www.w3.org/2000/svg"
            >
              <path
                d="M13.5 7.5l-5.793 5.793a3.5 3.5 0 01-4.95-4.95L9.05 2.05a2.25 2.25 0 013.182 3.182L5.94 11.525a1 1 0 01-1.414-1.414L10.818 3.818"
                stroke="currentColor"
                stroke-width="1.3"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
          <button class="cmd-menu-btn" onclick={toggleCommandMenu} {disabled} title="Commands">
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
