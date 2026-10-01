<script lang="ts">
  import { formatDiagnosticLocation } from "../../lib/diagnostics";
  import { Icon } from "../../lib/icons";
  import type { Diagnostic } from "../../lib/types";

  // A config file as text: line numbers beside it, with the lines that have
  // a problem marked, and the problems listed under it. A problem with a
  // position moves the cursor there.

  interface Props {
    /** The file's name, for the text box's label and the problem list. */
    name: string;
    value: string;
    readonly: boolean;
    problems: readonly Diagnostic[];
    /** One quiet line about the problems: checking, none found, or why they couldn't be checked. */
    status: string;
    oninput: (text: string) => void;
  }

  let { name, value, readonly, problems, status, oninput }: Props = $props();

  let area = $state<HTMLTextAreaElement>();
  let scrollTop = $state(0);

  const lineCount = $derived(value.split("\n").length);
  /** Each line with a problem, and its worst severity. */
  const marked = $derived.by(() => {
    const lines: Partial<Record<number, Diagnostic["severity"]>> = {};
    for (const { location, severity } of problems) {
      if (location?.kind !== "line" && location?.kind !== "line_column") continue;
      if (lines[location.line] !== "error") lines[location.line] = severity;
    }
    return lines;
  });

  function jump(line: number, column: number): void {
    if (area === undefined) return;
    const before = value.split("\n").slice(0, line - 1);
    const offset = before.reduce((sum, text) => sum + text.length + 1, 0) + column - 1;
    area.focus();
    area.setSelectionRange(offset, offset);
    const lineHeight = Number.parseFloat(getComputedStyle(area).lineHeight) || 0;
    area.scrollTop = Math.max(0, (line - 4) * lineHeight);
  }
</script>

<div class="editor" data-readonly={readonly || undefined}>
  <div class="gutter" aria-hidden="true">
    <div class="gutter-lines" style:transform="translateY({-scrollTop}px)">
      {#each { length: lineCount }, index (index)}
        <span data-problem={marked[index + 1]}>{index + 1}</span>
      {/each}
    </div>
  </div>
  <textarea
    bind:this={area}
    aria-label="Contents of {name}"
    spellcheck="false"
    autocapitalize="off"
    wrap="off"
    {readonly}
    {value}
    oninput={(event) => {
      oninput(event.currentTarget.value);
    }}
    onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}
  ></textarea>
</div>
<p class="status" aria-live="polite">{status}</p>
{#if problems.length > 0}
  <ul class="problems" aria-label="Problems in {name}">
    {#each problems as problem, index (index)}
      {@const where = formatDiagnosticLocation(problem.location)}
      {@const location = problem.location}
      <li class="problem" data-severity={problem.severity}>
        <Icon name="warning" size={13} />
        {#if location?.kind === "line" || location?.kind === "line_column"}
          <button
            type="button"
            class="problem-where"
            aria-label="Go to {where}"
            onclick={() => {
              jump(location.line, location.kind === "line_column" ? location.column : 1);
            }}>{where}</button
          >
        {:else if where}
          <code class="problem-where">{where}</code>
        {/if}
        <span class="problem-text">{problem.message}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .editor {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    overflow: hidden;
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-message);

    &:focus-within {
      border-color: var(--color-vein);
      box-shadow: 0 0 0 3px var(--color-vein-faint);
    }
  }

  /* Every line number is drawn, so the gutter takes no height of its own: the text box sets the row's. */
  .gutter {
    height: 0;
    min-height: 100%;
    overflow: hidden;
    border-right: 1px solid var(--color-line-soft);
    color: var(--color-text-3);
    text-align: right;
    user-select: none;
  }

  .gutter-lines {
    display: flex;
    flex-direction: column;
    padding: var(--space-12) var(--space-8) var(--space-12) var(--space-12);
    font-variant-numeric: tabular-nums;

    & [data-problem="error"] {
      color: var(--color-err-text);
      font-weight: var(--font-weight-medium);
    }

    & [data-problem="warning"] {
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }

  textarea {
    height: min(56vh, 460px);
    min-height: 160px;
    padding: var(--space-12);
    border: 0;
    outline: none;
    background: none;
    color: var(--color-text);
    font: inherit;
    resize: vertical;
    tab-size: 2;

    &[readonly] {
      color: var(--color-text-2);
    }
  }

  .status {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .problems {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    list-style: none;
  }

  .problem {
    display: flex;
    align-items: baseline;
    gap: var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);

    & > :global(svg) {
      flex: none;
      align-self: center;
    }

    &[data-severity="error"] > :global(svg) {
      color: var(--color-err-text);
    }
  }

  .problem-where {
    flex: none;
    padding: 0;
    border: 0;
    background: none;
    color: var(--color-text);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    white-space: nowrap;
  }

  button.problem-where {
    color: var(--color-vein-bright);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }

  .problem-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  @media (max-width: 760px) {
    .editor {
      font-size: var(--font-size-field-phone);
    }
  }
</style>
