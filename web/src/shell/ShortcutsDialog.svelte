<script lang="ts">
  import { Dialog, Kbd } from "../lib/ui";

  // The Keyboard shortcuts dialog: every key the app answers, where it works.

  let { open = $bindable(false) }: { open?: boolean } = $props();

  interface Shortcut {
    keys: readonly (readonly string[])[];
    does: string;
  }

  const SECTIONS: readonly { heading: string; shortcuts: readonly Shortcut[] }[] = [
    {
      heading: "Anywhere",
      shortcuts: [
        { keys: [["Mod", "K"]], does: "Search agents, places, settings and actions" },
        { keys: [["?"]], does: "Show these shortcuts, unless you're typing" },
        { keys: [["Esc"]], does: "Close the menu or dialog on top" },
      ],
    },
    {
      heading: "In the message box",
      shortcuts: [
        { keys: [["Enter"]], does: "Send. On a touch screen, Enter starts a new line instead" },
        { keys: [["Shift", "Enter"]], does: "Start a new line" },
        {
          keys: [["/"]],
          does: "As the first character, list the chat actions, such as Summarize older messages now. Keep typing to narrow them; Tab fills one in, Enter runs it.",
        },
        { keys: [["Esc"]], does: "Press twice to stop the reply while the agent is replying" },
      ],
    },
    {
      heading: "In lists and menus",
      shortcuts: [
        { keys: [["↑"], ["↓"]], does: "Move between rows" },
        { keys: [["Enter"], ["Space"]], does: "Open the row, or an agent's places in the sidebar" },
        { keys: [["Home"], ["End"]], does: "Go to the first or last row" },
      ],
    },
  ];
</script>

<Dialog bind:open title="Keyboard shortcuts" size="md" fullscreenOnPhone>
  {#each SECTIONS as section (section.heading)}
    <section class="shortcuts-section">
      <h3 class="shortcuts-heading">{section.heading}</h3>
      <dl class="shortcuts-list">
        {#each section.shortcuts as shortcut (shortcut.does)}
          <div class="shortcuts-row">
            <dt class="shortcuts-keys">
              {#each shortcut.keys as keys, at (at)}
                <Kbd {keys} />
              {/each}
            </dt>
            <dd class="shortcuts-does">{shortcut.does}</dd>
          </div>
        {/each}
      </dl>
    </section>
  {/each}
</Dialog>

<style>
  .shortcuts-section + .shortcuts-section {
    margin-top: var(--space-20);
  }

  .shortcuts-heading {
    margin-bottom: var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
  }

  .shortcuts-row {
    display: grid;
    grid-template-columns: 112px minmax(0, 1fr);
    gap: var(--space-12);
    align-items: baseline;
    padding: var(--space-6) 0;
  }

  .shortcuts-keys {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4);
  }

  .shortcuts-does {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }
</style>
