<script lang="ts">
  import ComposerFrame from "../../composer/ComposerFrame.svelte";
  import type { AppAction } from "../../lib/action-registry.svelte";
  import { runCommandLine } from "../../lib/command-line";
  import { composerClearance } from "../../lib/composer-clearance.svelte";
  import { Icon } from "../../lib/icons";
  import { IconButton } from "../../lib/ui";

  interface Props {
    value: string;
    label: string;
    placeholder: string;
    sendLabel: string;
    sending: boolean;
    blocked: string | null;
    actions?: readonly AppAction[];
    onsend: () => void;
  }

  let {
    value = $bindable(),
    label,
    placeholder,
    sendLabel,
    sending,
    blocked,
    actions,
    onsend,
  }: Props = $props();

  const uid = $props.id();
  const menu = $derived(
    actions === undefined || actions.length === 0
      ? undefined
      : { label: "Session actions", actions },
  );

  function send(): boolean {
    if (blocked !== null || sending || value.trim() === "") return false;
    const outcome = actions === undefined ? "message" : runCommandLine(actions, value);
    if (outcome === "refused") return false;
    if (outcome === "ran") value = "";
    else onsend();
    return true;
  }
</script>

<div class="session-composer" {@attach composerClearance.track}>
  <ComposerFrame
    bind:value
    {label}
    {placeholder}
    {menu}
    disabled={blocked !== null}
    describedby={blocked === null ? undefined : `${uid}-why`}
    onsend={send}
  >
    {#snippet sendControl()}
      <!-- A press leaves focus in the message box: on a phone that keeps the keyboard up. -->
      <IconButton
        icon="send"
        label={sendLabel}
        type="submit"
        variant="primary"
        loading={sending}
        disabled={blocked !== null || value.trim() === ""}
        onmousedown={(event) => event.preventDefault()}
      />
    {/snippet}
  </ComposerFrame>
  {#if blocked !== null}
    <p class="session-composer-why" id="{uid}-why">
      <Icon name="info" size={13} />{blocked}
    </p>
  {/if}
</div>

<style>
  .session-composer {
    flex: none;
    padding: var(--space-8) var(--space-12) var(--space-12);
  }

  .session-composer-why {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    max-width: var(--layout-reading-width);
    margin: var(--space-6) auto 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
