<script lang="ts">
  import type { CommandDef } from "../lib/commands";

  let {
    commands,
    selectedIndex,
    onSelect,
  }: {
    commands: CommandDef[];
    selectedIndex: number;
    onSelect: (name: string) => void;
  } = $props();
</script>

<div class="cmd-menu" role="listbox">
  {#each commands as cmd, i (cmd.name)}
    <div
      class="cmd-item"
      class:selected={i === selectedIndex}
      role="option"
      tabindex="-1"
      aria-selected={i === selectedIndex}
      onmousedown={(e: MouseEvent) => {
        e.preventDefault();
        onSelect(cmd.name);
      }}
      onmouseenter={() => (selectedIndex = i)}
    >
      <span class="cmd-name">{cmd.name}</span>
      <span class="cmd-desc">{cmd.description}</span>
    </div>
  {/each}
</div>
