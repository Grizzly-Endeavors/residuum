<script lang="ts">
  import { filterCommands, COMMAND_REGISTRY } from "../lib/commands";
  import CommandMenu from "./CommandMenu.svelte";
  import ModelSelector from "./ModelSelector.svelte";
  import ThinkingSelector from "./ThinkingSelector.svelte";

  let { onSend, disabled = false }: { onSend: (text: string) => void; disabled?: boolean } =
    $props();
  let value = $state("");
  let textarea: HTMLTextAreaElement | undefined = $state();

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

  function submit() {
    const text = value.trim();
    if (!text || disabled) return;
    onSend(text);
    value = "";
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
        ></textarea>
        <button class="send-btn" onclick={submit} disabled={disabled || !value.trim()}>Send</button>
      </div>
      <div class="chat-toolbar">
        <div class="chat-toolbar-left">
          <button class="cmd-menu-btn" onclick={toggleCommandMenu} {disabled} title="Commands">
            /
          </button>
        </div>
        <div class="chat-toolbar-right">
          <ModelSelector {disabled} />
          <ThinkingSelector {disabled} />
          <button class="send-btn-toolbar" onclick={submit} disabled={disabled || !value.trim()}
            >Send</button
          >
        </div>
      </div>
    </div>
  </div>
</div>
