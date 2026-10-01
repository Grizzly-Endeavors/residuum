<script lang="ts">
  import { Icon } from "../lib/icons";
  import { openSessionByAddress } from "../lib/session-address";
  import { Spinner } from "../lib/ui";
  import { stepText, type ActivityStep } from "./activity";
  import { openPathInPanel, pathHref } from "./feed-links";
  import StepDetail from "./StepDetail.svelte";

  // One step of a turn's activity line: what it did and to what, and while
  // the turn is watched, how it is going. Pressing the row opens its details.
  // A path target links into the context panel and a session target opens
  // the session, so those sit beside the row's button, not inside it.

  let { step, agent }: { step: ActivityStep; agent: string } = $props();

  const uid = $props.id();
  let open = $state(false);

  const STATUS_WORDS: Partial<Record<ActivityStep["status"], string>> = {
    running: "Running",
    failed: "Failed",
    stopped: "Stopped",
  };
  const statusWord = $derived(STATUS_WORDS[step.status]);
  const linked = $derived(step.target?.kind === "path" || step.target?.kind === "session");

  function openTarget(event: MouseEvent): void {
    const target = step.target;
    if (target === null) return;
    if (target.kind === "session") {
      void openSessionByAddress(agent, target.text, null);
      return;
    }
    const newTab = event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey;
    if (newTab) return;
    event.preventDefault();
    openPathInPanel(agent, target.text);
  }
</script>

<li class="step" data-status={step.status}>
  <div class="step-row">
    <span class="step-icon">
      {#if step.status === "running"}
        <Spinner size={11} />
      {:else if step.status === "failed"}
        <Icon name="warning" size={14} />
      {:else if step.status === "stopped"}
        <Icon name="close" size={14} />
      {:else}
        <Icon name={step.icon} size={14} />
      {/if}
    </span>
    <button
      type="button"
      class="step-toggle"
      aria-expanded={open}
      aria-controls="{uid}-detail"
      aria-label={statusWord ? `${stepText(step)}, ${statusWord.toLowerCase()}` : stepText(step)}
      onclick={() => (open = !open)}
    >
      <span>{step.verb}</span>
      {#if step.target && !linked}
        <span class="step-target" data-kind={step.target.kind}>
          {step.target.kind === "query" ? `“${step.target.text}”` : step.target.text}
        </span>
      {/if}
    </button>
    {#if step.target && linked}
      {#if step.target.kind === "path"}
        <a class="step-link" href={pathHref(agent, step.target.text)} onclick={openTarget}
          >{step.target.text}</a
        >
      {:else}
        <button type="button" class="step-link" onclick={openTarget}>{step.target.text}</button>
      {/if}
    {/if}
    {#if statusWord && step.status !== "running"}
      <span class="step-status" aria-hidden="true">{statusWord}</span>
    {/if}
    <span class="step-chevron" aria-hidden="true"><Icon name="chevron-down" size={14} /></span>
  </div>
  {#if open}
    <div class="step-detail" id="{uid}-detail">
      <StepDetail call={step.call} />
    </div>
  {/if}
</li>

<style>
  .step {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  /* The row's button stretches over the whole row; a target link sits above it. */
  .step-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-height: 28px;
    padding: var(--space-2) var(--space-6);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    transition:
      background var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-2);
      color: var(--color-text);
    }

    &:has(.step-toggle:focus-visible) {
      outline: var(--focus-outline-width) solid var(--color-vein-bright);
      outline-offset: calc(-1 * var(--focus-outline-width));
    }
  }

  [data-status="running"] .step-row {
    color: var(--color-text);
  }

  .step-icon {
    display: grid;
    flex: none;
    place-items: center;
    width: 16px;
    height: 16px;
    color: var(--color-text-3);

    [data-status="running"] & {
      color: var(--color-vein-bright);
    }

    [data-status="failed"] & {
      color: var(--color-err-text);
    }
  }

  .step-toggle {
    min-width: 0;
    text-align: start;
    overflow-wrap: anywhere;

    &::after {
      content: "";
      position: absolute;
      inset: 0;
      border-radius: inherit;
    }

    &:focus-visible {
      outline: none;
    }
  }

  .step-target {
    &[data-kind="code"] {
      font-family: var(--font-code);
      font-size: var(--font-size-code);
    }
  }

  .step-link {
    position: relative;
    min-width: 0;
    margin-left: calc(-1 * var(--space-4));
    color: var(--color-vein-bright);
    font-family: var(--font-code);
    font-size: var(--font-size-code);
    text-align: start;
    overflow-wrap: anywhere;

    &:hover {
      text-decoration: underline;
    }
  }

  .step-status {
    flex: none;
    font-size: var(--font-size-xs);

    [data-status="failed"] & {
      color: var(--color-err-text);
    }

    [data-status="stopped"] & {
      color: var(--color-text-3);
    }
  }

  .step-chevron {
    display: grid;
    flex: none;
    margin-left: auto;
    color: var(--color-text-3);
    transition: transform var(--duration-base) var(--ease-out);

    .step-row:has([aria-expanded="true"]) & {
      transform: rotate(180deg);
    }
  }

  /* Under the label, past the row's padding, icon and gap. */
  .step-detail {
    margin: var(--space-2) 0 var(--space-6) calc(var(--space-6) + var(--space-16) + var(--space-8));
  }

  @media (max-width: 760px) {
    .step-row {
      min-height: var(--layout-touch-target);
    }

    .step-detail {
      margin-left: var(--space-8);
    }
  }
</style>
