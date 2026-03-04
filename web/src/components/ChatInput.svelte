<script lang="ts">
  let { onSend, disabled = false }: { onSend: (text: string) => void; disabled?: boolean } =
    $props();
  let value = $state("");
  let textarea: HTMLTextAreaElement | undefined = $state();

  function autoResize() {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 160)}px`;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  function submit() {
    const text = value.trim();
    if (!text || disabled) return;
    onSend(text);
    value = "";
    if (textarea) textarea.style.height = "auto";
  }

  $effect(() => {
    // trigger resize when value changes
    value;
    autoResize();
  });
</script>

<div class="chat-input-area">
  <div class="chat-input-wrap">
    <textarea
      bind:this={textarea}
      bind:value
      class="chat-input"
      placeholder="Send a message..."
      rows="1"
      {disabled}
      onkeydown={handleKeydown}
      oninput={autoResize}
    ></textarea>
    <button class="send-btn" onclick={submit} disabled={disabled || !value.trim()}> Send </button>
  </div>
</div>
