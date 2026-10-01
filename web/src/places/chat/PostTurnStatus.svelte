<script lang="ts" module>
  /** What the agent is doing after its reply, in words, or null when nothing. */
  export function postTurnWords(memory: boolean, review: boolean): string | null {
    if (memory && review)
      return "Noting what matters from this conversation and reviewing the last reply";
    if (memory) return "Noting what matters from this conversation";
    if (review) return "Reviewing the last reply";
    return null;
  }
</script>

<script lang="ts">
  import { Icon } from "../../lib/icons";

  // The quiet line under the last reply while the agent's memory work after a
  // turn runs (design §4). It never holds up the next message.

  let { memory, review }: { memory: boolean; review: boolean } = $props();

  const words = $derived(postTurnWords(memory, review));
</script>

<div class="post-turn" role="status">
  {#if words !== null}
    <Icon name="memory" size={13} />
    <span>{words}</span>
  {/if}
</div>

<style>
  .post-turn {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    /* Close under the reply it follows, and taking no room in the feed while empty. */
    margin-top: calc(-1 * var(--space-10));
    color: var(--color-text-3);
    font-size: var(--font-size-sm);

    & :global(svg) {
      flex: none;
      animation: post-turn-pulse var(--duration-pulse) var(--ease-in-out) infinite;
    }
  }

  .post-turn:empty {
    margin-top: calc(-1 * var(--space-18));
  }

  @keyframes post-turn-pulse {
    50% {
      opacity: 0.4;
    }
  }
</style>
