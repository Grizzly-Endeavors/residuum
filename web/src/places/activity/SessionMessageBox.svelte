<script lang="ts">
  import { composerClearance } from "../../lib/composer-clearance.svelte";
  import { Icon } from "../../lib/icons";
  import { IconButton } from "../../lib/ui";

  // The message box at the foot of a session panel: a box that grows with its
  // text, Enter to send and Shift+Enter for a new line, and the reason under
  // it while nothing can be sent.

  interface Props {
    value: string;
    /** The box's name, such as "Message this session". */
    label: string;
    placeholder: string;
    /** The send button's name. */
    sendLabel: string;
    sending: boolean;
    /** Why nothing can be sent now, such as "Start atlas first.", or null when it can. */
    blocked: string | null;
    onsend: () => void;
  }

  let {
    value = $bindable(),
    label,
    placeholder,
    sendLabel,
    sending,
    blocked,
    onsend,
  }: Props = $props();

  const uid = $props.id();

  function grow(box: HTMLTextAreaElement): void {
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, 160))}px`;
  }

  function sendOnEnter(event: KeyboardEvent): void {
    if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
    event.preventDefault();
    if (blocked === null) onsend();
  }
</script>

<form
  class="session-composer"
  {@attach composerClearance.track}
  onsubmit={(event) => {
    event.preventDefault();
    onsend();
  }}
>
  <div class="session-composer-box">
    <textarea
      aria-label={label}
      rows="1"
      {placeholder}
      disabled={blocked !== null}
      aria-describedby={blocked === null ? undefined : `${uid}-why`}
      bind:value
      oninput={(event) => grow(event.currentTarget)}
      onkeydown={sendOnEnter}
    ></textarea>
    <IconButton
      icon="send"
      label={sendLabel}
      type="submit"
      variant="primary"
      size="sm"
      loading={sending}
      disabled={blocked !== null || value.trim() === ""}
    />
  </div>
  {#if blocked !== null}
    <p class="session-composer-why" id="{uid}-why">
      <Icon name="info" size={13} />{blocked}
    </p>
  {/if}
</form>

<style>
  .session-composer {
    flex: none;
    padding: var(--space-8) var(--space-12) var(--space-12);
    border-top: 1px solid var(--color-line-soft);
  }

  .session-composer-box {
    display: flex;
    align-items: flex-end;
    gap: var(--space-8);
    padding: var(--space-6) var(--space-6) var(--space-6) var(--space-12);
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-lg);
    background: var(--color-input);
    transition: border-color var(--duration-fast) var(--ease-out);

    &:focus-within {
      border-color: var(--color-vein);
    }

    & textarea {
      flex: 1;
      min-width: 0;
      max-height: 160px;
      padding: var(--space-4) 0;
      border: 0;
      background: transparent;
      color: var(--color-text);
      font-size: var(--font-size-ui);
      line-height: var(--line-height-ui);
      resize: none;

      &:focus-visible {
        outline: none;
      }

      &:disabled {
        cursor: not-allowed;
      }
    }
  }

  .session-composer-why {
    display: flex;
    align-items: center;
    gap: var(--space-6);
    margin-top: var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  @media (max-width: 760px) {
    .session-composer-box textarea {
      font-size: var(--font-size-field-phone);
    }
  }
</style>
