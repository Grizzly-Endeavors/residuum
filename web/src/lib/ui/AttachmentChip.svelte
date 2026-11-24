<script lang="ts">
  import { Icon } from "../icons";
  import { fileSize } from "../file-size";
  import { ATTACHMENT_ICONS, attachmentKind, splitFilename } from "./attachment-file";
  import Button from "./Button.svelte";
  import IconButton from "./IconButton.svelte";
  import Spinner from "./Spinner.svelte";
  import VisuallyHidden from "./VisuallyHidden.svelte";
  import type { AttachmentState } from "./types";

  interface Props {
    filename: string;
    size?: number;
    contentType?: string;
    state?: AttachmentState;
    progress?: number;
    pendingLabel?: string;
    failure?: string;
    href?: string;
    download?: string;
    onopen?: () => void;
    onremove?: () => void;
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
    <span class="ui-attachment-failure" role="status">
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
      aria-label={`Try again ${filename}`}
      onclick={() => onretry?.()}
    >
      Try again
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
    .ui-attachment {
      min-height: calc(var(--layout-touch-target) + 2px);
    }

    .ui-attachment-download {
      width: var(--layout-touch-target);
      height: var(--layout-touch-target);
    }
  }
</style>
