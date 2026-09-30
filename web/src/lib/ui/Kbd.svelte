<script lang="ts">
  import VisuallyHidden from "./VisuallyHidden.svelte";

  // A key or key combination. "Mod" is Command on Apple devices and Ctrl
  // elsewhere; on Apple devices the modifiers draw as their symbols.
  interface Props {
    keys: readonly string[];
    /** Which keyboard to draw for. Defaults to the one this device has. */
    platform?: "apple" | "other";
  }

  let { keys, platform = isApple() ? "apple" : "other" }: Props = $props();

  function isApple(): boolean {
    return /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);
  }

  const APPLE_SYMBOLS: Readonly<Record<string, { symbol: string; name: string }>> = {
    Mod: { symbol: "⌘", name: "Command" },
    Alt: { symbol: "⌥", name: "Option" },
    Shift: { symbol: "⇧", name: "Shift" },
    Ctrl: { symbol: "⌃", name: "Control" },
  };

  const shown = $derived(
    keys.map((key) => {
      if (platform === "apple") return APPLE_SYMBOLS[key] ?? { symbol: key, name: key };
      const word = key === "Mod" ? "Ctrl" : key;
      return { symbol: word, name: word };
    }),
  );
</script>

<kbd class="ui-kbd" data-platform={platform}>
  {#each shown as key, index (index)}
    {#if key.symbol === key.name}
      <kbd>{key.symbol}</kbd>
    {:else}
      <kbd
        ><span aria-hidden="true">{key.symbol}</span><VisuallyHidden>{key.name}</VisuallyHidden
        ></kbd
      >
    {/if}
  {/each}
</kbd>

<style>
  .ui-kbd {
    position: relative;
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-2) var(--space-6);
    border-radius: var(--corner-sm);
    background: var(--color-stone-3);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    line-height: var(--line-height-tight);
    white-space: nowrap;

    & > kbd {
      font: inherit;
    }
  }

  /* Symbols sit together, as printed on the keys: ⌘K. */
  .ui-kbd[data-platform="apple"] {
    gap: var(--space-2);
  }
</style>
