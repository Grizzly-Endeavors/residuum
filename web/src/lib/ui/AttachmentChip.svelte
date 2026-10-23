<script lang="ts">
  import { Icon } from "../icons";
  import { fileSize } from "../file-size";
  import { ATTACHMENT_ICONS, attachmentKind, splitFilename } from "./attachment-file";
  import Button from "./Button.svelte";
  import IconButton from "./IconButton.svelte";
  import Spinner from "./Spinner.svelte";
  import VisuallyHidden from "./VisuallyHidden.svelte";
  import type { AttachmentState } from "./types";

  // A file as a chip: its icon, name and size, in an open message, a list row,
  // or the composer's staged files. A ready chip is a download link (`href`),
  // an Open button (`onopen`), or plain text, and may carry Remove. While the
  // file is pending or failed it can't be opened or downloaded; a failed chip
  // says so and offers Retry.
  //
  // The chip has a `control-border` outline over a stone fill, so it stays
  // visible on any container stone (stone-0 to stone-3) and its boundary meets
  // the 3:1 a control needs. A failed chip swaps the outline for `err` and the
  // fill for `err-tint`.
  interface Props {
    filename: string;
    /** Bytes. Shown beside the name when given. */
    size?: number;
    /** The file's MIME type. With the filename's extension it picks the icon. */
    contentType?: string;
    state?: AttachmentState;
    /** How far a pending file has got, 0 to 1. Without it a pending chip shows a spinner. */
    progress?: number;
    /** What a pending file is doing, as a verb that reads before the filename ("Uploading"). */
    pendingLabel?: string;
    /** What went wrong, in plain words, for a failed chip ("Couldn't upload"). */
    failure?: string;
    /** Makes a ready chip a download link to this address. */
    href?: string;
    /** The name the browser saves the download under. Defaults to the filename. */
    download?: string;
    /** Makes a ready chip an Open button. With `href` too, the download becomes a second control. */
    onopen?: () => void;
    /** Adds a Remove button. */
    onremove?: () => void;
    /** Adds a Retry button to a failed chip. */
    onretry?: () => void;
    class?: string;
  }

  let {
    filename,
    size,
    contentType,
    state = "ready",
    progress,
    pendingLabel = "Uploading",
    failure = "Failed",
    href,
    download,
    onopen,
    onremove,
    onretry,
    class: className,
  }: Props = $props();

  const uid = $props.id();
  const sizeId = `${uid}-size`;

  const kind = $derived(attachmentKind(filename, contentType));
  const name = $derived(splitFilename(filename));
  const percent = $derived(
    progress === undefined ? undefined : Math.round(Math.min(Math.max(progress, 0), 1) * 100),
  );

  const ready = $derived(state === "ready");
  const opens = $derived(ready && onopen !== undefined);
  // With both, Open is the chip and the download is a trailing control.
  const downloads = $derived(ready && href !== undefined);
  const main = $derived.by<"open" | "download" | "text">(() => {
    if (opens) return "open";
    return downloads ? "download" : "text";
  });
  const retries = $derived(state === "failed" && onretry !== undefined);
  const hasActions = $derived(retries || onremove !== undefined || (opens && downloads));
</script>

{#snippet body()}
  {#if state === "pending" && percent === undefined}
    <span
      class="ui-attachment-icon"
      data-tone="busy"
      role="progressbar"
      aria-label={`${pendingLabel} ${filename}`}
    >
      <Spinner size={13} />
    </span>
  {:else if state === "failed"}
    <span class="ui-attachment-icon" data-tone="danger"><Icon name="warning" size={14} /></span>
  {:else}
    <span class="ui-attachment-icon"><Icon name={ATTACHMENT_ICONS[kind]} size={14} /></span>
  {/if}
  <span class="ui-attachment-name">
    <!-- Drawn in two pieces so the extension survives truncation; read as one name. -->
    <span class="ui-attachment-pieces" aria-hidden="true">
      <span class="ui-attachment-stem">{name.stem}</span>
      {#if name.extension}<span class="ui-attachment-extension">{name.extension}</span>{/if}
    </span>
    <VisuallyHidden>{filename}</VisuallyHidden>
  </span>
  {#if state === "failed"}
    <span class="ui-attachment-failure" role="alert">
      <VisuallyHidden>{`${filename}: `}</VisuallyHidden>{failure}
    </span>
  {:else if size !== undefined}
    <span class="ui-attachment-size" id={sizeId}>{fileSize(size)}</span>
  {/if}
  {#if main === "download"}
    <span class="ui-attachment-glyph"><Icon name="arrow-down" size={14} /></span>
  {:else if main === "open"}
    <span class="ui-attachment-glyph"><Icon name="chevron-right" size={14} /></span>
  {/if}
{/snippet}

<span
  class={["ui-attachment", className]}
  data-state={state}
  data-kind={kind}
  data-actions={hasActions || undefined}
>
  {#if main === "open"}
    <button
      type="button"
      class="ui-attachment-main"
      aria-label={`Open ${filename}`}
      aria-describedby={size === undefined ? undefined : sizeId}
      onclick={() => onopen?.()}
    >
      {@render body()}
    </button>
  {:else if main === "download"}
    <a
      class="ui-attachment-main"
      {href}
      download={download ?? filename}
      aria-label={`Download ${filename}`}
      aria-describedby={size === undefined ? undefined : sizeId}
    >
      {@render body()}
    </a>
  {:else}
    <span class="ui-attachment-main">{@render body()}</span>
  {/if}

  {#if percent !== undefined && state === "pending"}
    <span
      class="ui-attachment-progress"
      role="progressbar"
      aria-label={`${pendingLabel} ${filename}`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
    >
      <span class="ui-attachment-progress-fill" style:width="{percent}%"></span>
    </span>
  {/if}

  {#if opens && downloads}
    <a
      class="ui-attachment-download"
      {href}
      download={download ?? filename}
      aria-label={`Download ${filename}`}
    >
      <Icon name="arrow-down" size={14} />
    </a>
  {/if}
  {#if retries}
    <Button
      class="ui-attachment-action"
      variant="quiet"
      size="sm"
      icon="reload"
      aria-label={`Retry ${filename}`}
      onclick={() => onretry?.()}
    >
      Retry
    </Button>
  {/if}
  {#if onremove}
    <IconButton
      class="ui-attachment-action"
      icon="close"
      size="sm"
      label={`Remove ${filename}`}
      onclick={() => onremove?.()}
    />
  {/if}
</span>

<style>
  .ui-attachment {
    position: relative;
    display: inline-flex;
    align-items: center;
    max-width: min(100%, 360px);
    min-height: 32px;
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-stone-3);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    /* A quiet button on the chip lifts to stone-4; stone-3 is the chip's own fill. */
    --ui-quiet-hover: var(--color-stone-4);
  }

  .ui-attachment[data-actions] {
    padding-right: var(--space-2);
  }

  .ui-attachment[data-state="failed"] {
    border-color: var(--color-err);
    background: var(--color-err-tint);
  }

  .ui-attachment-main {
    display: flex;
    flex: 1 1 auto;
    align-items: center;
    align-self: stretch;
    gap: var(--space-8);
    min-width: 0;
    padding: 0 var(--space-10);
    border-radius: calc(var(--corner-sm) - 1px);
    color: inherit;
    text-decoration: none;
    transition: background-color var(--duration-fast) var(--ease-out);
  }

  :is(a, button).ui-attachment-main {
    cursor: pointer;

    &:hover {
      background: var(--color-stone-4);
    }
  }

  /* The chip's own edge is the ring; an outset ring would poke out of it. */
  :is(a, button).ui-attachment-main:focus-visible,
  .ui-attachment-download:focus-visible,
  .ui-attachment :global(.ui-attachment-action:focus-visible) {
    outline-offset: calc(-1 * var(--focus-outline-width));
  }

  .ui-attachment-icon {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: 14px;
    height: 14px;
    color: var(--color-text-2);

    &[data-tone="busy"] {
      color: var(--color-vein-bright);
    }

    &[data-tone="danger"] {
      color: var(--color-err-text);
    }
  }

  /* The stem gives way first, so the extension stays readable. */
  .ui-attachment-name,
  .ui-attachment-pieces {
    display: flex;
    min-width: 0;
  }

  .ui-attachment-stem {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ui-attachment-extension {
    flex: none;
    white-space: nowrap;
  }

  .ui-attachment-size,
  .ui-attachment-failure {
    flex: none;
    font-size: var(--font-size-xs);
    white-space: nowrap;
  }

  .ui-attachment-size {
    color: var(--color-text-2);
    font-variant-numeric: tabular-nums;
  }

  .ui-attachment-glyph {
    display: inline-grid;
    flex: none;
    color: var(--color-text-2);
    transition: color var(--duration-fast) var(--ease-out);
  }

  :is(a, button).ui-attachment-main:hover .ui-attachment-glyph {
    color: var(--color-text);
  }

  .ui-attachment-download {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--ui-quiet-hover);
      color: var(--color-text);
    }
  }

  .ui-attachment-progress {
    position: absolute;
    right: 0;
    bottom: 0;
    left: 0;
    height: 2px;
    overflow: hidden;
    border-radius: 0 0 calc(var(--corner-sm) - 1px) calc(var(--corner-sm) - 1px);
  }

  .ui-attachment-progress-fill {
    display: block;
    height: 100%;
    background: var(--color-vein-bright);
    transition: width var(--duration-fast) var(--ease-out);
  }

  @media (max-width: 760px) {
    /* The border takes 2px of the touch target's height. */
    .ui-attachment {
      min-height: calc(var(--layout-touch-target) + 2px);
    }

    .ui-attachment-download {
      width: var(--layout-touch-target);
      height: var(--layout-touch-target);
    }
  }
</style>
