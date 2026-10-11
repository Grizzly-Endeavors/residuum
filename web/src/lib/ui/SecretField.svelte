<script lang="ts">
  import { tick } from "svelte";
  import { Icon } from "../icons";
  import Button from "./Button.svelte";
  import Field from "./Field.svelte";
  import IconButton from "./IconButton.svelte";
  import Input from "./Input.svelte";
  import type { SecretSource } from "./types";

  // A key or token that is never shown once saved. With a saved value it
  // says where the value lives, and Change (stored) or Replace (environment
  // variable) opens a box for a new one; Cancel closes it again. Without one,
  // the box is open from the start. The typed value is the caller's to store.
  interface Props {
    label: string;
    source: SecretSource;
    /** What the user typed as the new value. */
    value?: string;
    /** Whether the box for a new value is open over a saved one. */
    editing?: boolean;
    hint?: string;
    error?: string;
    placeholder?: string;
    disabled?: boolean;
    oncancel?: () => void;
  }

  let {
    label,
    source,
    value = $bindable(""),
    editing = $bindable(false),
    hint,
    error,
    placeholder,
    disabled = false,
    oncancel,
  }: Props = $props();

  const typing = $derived(source.kind === "none" || editing);
  const replaceVerb = $derived(source.kind === "env" ? "Replace" : "Change");
  let revealed = $state(false);
  let entry = $state<HTMLInputElement | HTMLTextAreaElement>();
  let change = $state<HTMLButtonElement>();

  async function startReplacing(): Promise<void> {
    editing = true;
    await tick();
    entry?.focus();
  }

  async function cancel(): Promise<void> {
    value = "";
    editing = false;
    revealed = false;
    oncancel?.();
    await tick();
    change?.focus();
  }
</script>

<Field {label} {hint} {error} labelElement={typing ? "label" : "span"}>
  {#snippet children(control)}
    {#if typing}
      <span class="ui-secret-entry">
        <Input
          bind:value
          bind:element={entry}
          id={control.id}
          type={revealed ? "text" : "password"}
          code={revealed}
          autocomplete="off"
          spellcheck="false"
          {placeholder}
          {disabled}
          invalid={control.invalid}
          aria-describedby={control.describedBy}
        />
        <IconButton
          icon={revealed ? "eye-off" : "eye"}
          label="Show {label}"
          pressed={revealed}
          {disabled}
          onclick={() => {
            revealed = !revealed;
          }}
        />
        {#if source.kind !== "none"}
          <Button variant="quiet" onclick={() => void cancel()}>Cancel</Button>
        {/if}
      </span>
    {:else}
      <div
        class="ui-secret-saved"
        role="group"
        aria-labelledby={control.labelId}
        aria-describedby={control.describedBy}
      >
        <Icon name="key" size={15} />
        <span class="ui-secret-status">
          {#if source.kind === "env"}
            From environment variable <code>{source.variable}</code>
          {:else}
            Stored securely
          {/if}
        </span>
        <!-- The visible verb, with the field's name for assistive technology. -->
        <Button
          variant="secondary"
          size="sm"
          {disabled}
          aria-label="{replaceVerb} {label}"
          bind:element={change}
          onclick={() => void startReplacing()}
        >
          {replaceVerb}
        </Button>
      </div>
    {/if}
  {/snippet}
</Field>

<style>
  .ui-secret-entry {
    display: flex;
    flex: 1;
    align-items: center;
    gap: var(--space-4);
    min-width: 0;
  }

  .ui-secret-saved {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-10);
    min-height: 36px;
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .ui-secret-status {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .ui-secret-status code {
    color: var(--color-text);
  }
</style>
