<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon, type IconName } from "../icons";
  import IconButton from "./IconButton.svelte";
  import Spinner from "./Spinner.svelte";
  import type { BannerTone } from "./types";

  // A notice at the top of a region: information on a blue wash, a warning or
  // an error on a red one. Errors are announced as alerts, the rest politely.
  interface Props {
    tone?: BannerTone;
    /** A short bold lead-in before the message. */
    title?: string;
    /** Replaces the tone's icon, for example `check` after a save. */
    icon?: IconName;
    /** Something is under way: a spinner takes the icon's place. */
    busy?: boolean;
    /** Square corners, for a banner that spans its region edge to edge. */
    edge?: boolean;
    /** Shows a close button that calls this. */
    ondismiss?: () => void;
    actions?: Snippet;
    children: Snippet;
  }

  let {
    tone = "info",
    title,
    icon,
    busy = false,
    edge = false,
    ondismiss,
    actions,
    children,
  }: Props = $props();

  const TONE_ICONS: Readonly<Record<BannerTone, IconName>> = {
    info: "info",
    warn: "warning",
    error: "warning",
  };
</script>

<div
  class="ui-banner"
  data-tone={tone}
  data-edge={edge || undefined}
  role={tone === "error" ? "alert" : "status"}
>
  <span class="ui-banner-icon">
    {#if busy}
      <Spinner size={13} />
    {:else}
      <Icon name={icon ?? TONE_ICONS[tone]} size={16} />
    {/if}
  </span>
  <p class="ui-banner-message">
    {#if title}<strong class="ui-banner-title">{title}</strong>{/if}
    {@render children()}
  </p>
  {#if actions}
    <div class="ui-banner-actions">{@render actions()}</div>
  {/if}
  {#if ondismiss}
    <IconButton icon="close" label="Dismiss" size="sm" onclick={ondismiss} />
  {/if}
</div>

<style>
  .ui-banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-10);
    padding: var(--space-8) var(--space-8) var(--space-8) var(--space-14);
    border-radius: var(--corner-md);
    color: var(--color-text);
    font-size: var(--font-size-sm);
  }

  .ui-banner[data-tone="info"] {
    background: var(--color-vein-faint);

    & > .ui-banner-icon {
      color: var(--color-vein-bright);
    }
  }

  .ui-banner:is([data-tone="warn"], [data-tone="error"]) {
    background: var(--color-err-tint);

    & > .ui-banner-icon {
      color: var(--color-err-text);
    }
  }

  .ui-banner[data-edge] {
    border-radius: 0;
  }

  /* Icon and message align to the message's first line when it wraps. */
  .ui-banner-icon {
    display: grid;
    flex: none;
    align-self: flex-start;
    place-items: center;
    width: 16px;
    height: 28px;
  }

  .ui-banner-message {
    flex: 1 1 200px;
    align-self: flex-start;
    min-width: 0;
    padding: var(--space-4) 0;
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  .ui-banner-title {
    margin-right: var(--space-6);
    font-weight: var(--font-weight-semibold);
  }

  .ui-banner-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
  }
</style>
