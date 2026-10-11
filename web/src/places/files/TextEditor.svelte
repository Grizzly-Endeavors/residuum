<script lang="ts">
  import { formatDiagnosticLocation } from "../../lib/diagnostics";
  import { Icon } from "../../lib/icons";
  import type { Diagnostic } from "../../lib/types";

  // A file as text with its problems listed under it; a problem with a
  // position moves the cursor there. The file panel fills itself with one,
  // its lines wrapping like prose. Raw config shows one as a field, its lines
  // unwrapped and numbered, with the lines that have a problem marked.

  interface Props {
    /** The file's name, for the text box's label and the problem list. */
    name: string;
    value: string;
    problems: readonly Diagnostic[];
    oninput: (text: string) => void;
    /** `panel` fills its container edge to edge; `field` is a bordered box with numbered lines. */
    layout: "panel" | "field";
    readonly?: boolean;
    /** One quiet line about the problems: being checked, none found, or why they couldn't be. */
    status?: string;
    onkeydown?: (event: KeyboardEvent) => void;
  }

  let {
    name,
    value,
    problems,
    oninput,
    layout,
    readonly = false,
    status,
    onkeydown,
  }: Props = $props();

  let area = $state<HTMLTextAreaElement>();
  let scrollTop = $state(0);

  const numbered = $derived(layout === "field");
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

<div class="text-editor" data-layout={layout}>
  <div class="text-editor-box" data-readonly={readonly || undefined}>
    {#if numbered}
      <div class="gutter" aria-hidden="true">
        <div class="gutter-lines" style:transform="translateY({-scrollTop}px)">
          {#each { length: lineCount }, index (index)}
            <span data-problem={marked[index + 1]}>{index + 1}</span>
          {/each}
        </div>
      </div>
    {/if}
    <textarea
      bind:this={area}
      aria-label="Contents of {name}"
      spellcheck="false"
      autocapitalize="off"
      wrap={numbered ? "off" : "soft"}
      {readonly}
      {value}
      {onkeydown}
      oninput={(event) => {
        oninput(event.currentTarget.value);
      }}
      onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}
    ></textarea>
  </div>
  {#if status !== undefined}
    <p class="status" aria-live="polite">{status}</p>
  {/if}
  {#if problems.length > 0}
    <ul class="problems" aria-label="Problems in {name}">
      {#each problems as problem, index (index)}
        {@const where = formatDiagnosticLocation(problem.location)}
        {@const location = problem.location}
        <li class="problem" data-severity={problem.severity}>
          <Icon name={problem.severity === "error" ? "warning" : "info"} size={14} />
          <span class="problem-text">
            {#if location?.kind === "line" || location?.kind === "line_column"}
              <button
                type="button"
                class="problem-where"
                onclick={() => {
                  jump(location.line, location.kind === "line_column" ? location.column : 1);
                }}>{where}</button
              >
            {:else if where}
              <code class="problem-where">{where}</code>
            {/if}
            {problem.message}
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .text-editor {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);

    &[data-layout="panel"] {
      flex: 1;
      gap: 0;
      min-height: 0;
    }
  }

  .text-editor-box {
    display: grid;
    flex: 1;
    grid-template-columns: auto minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr);
    min-height: 0;
    overflow: hidden;
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-message);
  }

  /* The page of the document: the panel's deepest surface, edge to edge under its header. */
  [data-layout="panel"] .text-editor-box {
    background: var(--color-stone-0);

    &:focus-within {
      box-shadow: inset 2px 0 0 var(--color-vein);
    }
  }

  [data-layout="field"] .text-editor-box {
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-input);

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
    grid-column: -2;
    width: 100%;
    padding: var(--space-12);
    border: 0;
    outline: none;
    background: none;
    color: var(--color-text);
    font: inherit;
    resize: none;
    tab-size: 2;

    &[readonly] {
      color: var(--color-text-2);
    }
  }

  [data-layout="panel"] textarea {
    height: 100%;
    padding: var(--space-14) var(--space-18);
  }

  [data-layout="field"] textarea {
    height: min(56vh, 460px);
    min-height: 160px;
    resize: vertical;
  }

  .status {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .problems {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-6);
    font-size: var(--font-size-sm);
    list-style: none;
  }

  [data-layout="panel"] .problems {
    gap: var(--space-4);
    max-height: 30%;
    padding: var(--space-10) var(--space-12);
    overflow-y: auto;
    border-top: 1px solid var(--color-line-soft);
  }

  .problem {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
    color: var(--color-text);

    & > :global(svg) {
      flex: none;
      margin-top: var(--space-2);
      color: var(--color-text-2);
    }

    &[data-severity="error"] > :global(svg) {
      color: var(--color-err-text);
    }
  }

  .problem-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .problem-where {
    margin-right: var(--space-6);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
  }

  button.problem-where {
    color: var(--color-vein-bright);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  @media (max-width: 760px) {
    .text-editor-box {
      font-size: var(--font-size-field-phone);
    }

    [data-layout="panel"] textarea {
      padding: var(--space-12) var(--space-16);
    }
  }
</style>
