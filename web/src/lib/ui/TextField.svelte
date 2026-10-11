<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";
  import Field from "./Field.svelte";
  import Input from "./Input.svelte";

  interface Props extends Omit<HTMLInputAttributes, "value" | "type" | "children" | "id"> {
    label: string;
    value?: string;
    hint?: string;
    error?: string;
    labelHidden?: boolean;
    type?: "text" | "email" | "url" | "search" | "password";
    multiline?: boolean;
    rows?: number;
    /** Code, paths and ids: JetBrains Mono. */
    code?: boolean;
    element?: HTMLInputElement | HTMLTextAreaElement;
  }

  let {
    label,
    value = $bindable(""),
    hint,
    error,
    labelHidden = false,
    type = "text",
    element = $bindable(),
    ...rest
  }: Props = $props();
</script>

<Field {label} {hint} {error} {labelHidden}>
  {#snippet children(control)}
    <Input
      bind:value
      bind:element
      {...rest}
      {type}
      id={control.id}
      invalid={control.invalid}
      aria-describedby={control.describedBy}
    />
  {/snippet}
</Field>
