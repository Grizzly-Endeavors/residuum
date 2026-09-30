<script lang="ts">
  import type { StatusDotState } from "./types";

  // An agent's state as a mark. Each state has its own shape as well as its
  // own color: a filled dot running, a ring stopped, a turning ring starting
  // (clockwise) or stopping (backwards), a triangle failed.
  interface Props {
    state: StatusDotState;
    /** A running agent that is busy with a turn: the dot turns blue and pulses. */
    working?: boolean;
    /** The state in words. Without it the mark is decorative and the words must sit beside it. */
    label?: string;
  }

  let { state, working = false, label }: Props = $props();
</script>

<span
  class="ui-status-dot"
  data-state={state}
  data-working={(working && state === "running") || undefined}
  role={label === undefined ? undefined : "img"}
  aria-label={label}
  aria-hidden={label === undefined ? "true" : undefined}
>
  <span class="ui-status-mark"></span>
</span>

<style>
  .ui-status-dot {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: 16px;
    height: 16px;
  }

  .ui-status-mark {
    display: block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  [data-state="running"] > .ui-status-mark {
    background: var(--color-moss-text);
  }

  [data-working] > .ui-status-mark {
    background: var(--color-vein-bright);
    box-shadow: 0 0 0 3px var(--color-vein-tint);
    animation: ui-status-pulse var(--duration-pulse) var(--ease-in-out) infinite;
  }

  [data-state="stopped"] > .ui-status-mark {
    border: 1.5px solid var(--color-text-3);
  }

  [data-state="starting"] > .ui-status-mark,
  [data-state="stopping"] > .ui-status-mark {
    border: 1.5px solid var(--color-vein-bright);
    border-right-color: transparent;
    animation: ui-status-turn var(--duration-spin) var(--ease-linear) infinite;
  }

  [data-state="stopping"] > .ui-status-mark {
    border-color: var(--color-text-2);
    border-left-color: transparent;
    animation-direction: reverse;
  }

  [data-state="failed"] > .ui-status-mark {
    width: 10px;
    height: 9px;
    border-radius: 0;
    background: var(--color-err-text);
    clip-path: polygon(50% 0, 100% 100%, 0 100%);
  }

  @keyframes ui-status-pulse {
    50% {
      box-shadow: 0 0 0 5px var(--color-vein-faint);
    }
  }

  @keyframes ui-status-turn {
    to {
      transform: rotate(360deg);
    }
  }
</style>
