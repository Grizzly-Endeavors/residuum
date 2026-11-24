<script lang="ts">
  import { envReferenceName, isSecretReference } from "../../lib/secrets";
  import { SecretField, type SecretSource } from "../../lib/ui";

  interface Props {
    label: string;
    value?: string;
    saved: string;
    hint?: string;
    error?: string;
    placeholder?: string;
  }

  let { label, value = $bindable(""), saved, hint, error, placeholder }: Props = $props();

  function sourceOf(text: string): SecretSource {
    if (isSecretReference(text)) return { kind: "stored" };
    const variable = envReferenceName(text);
    return variable === null ? { kind: "none" } : { kind: "env", variable };
  }

  let typed = $state("");
  let editing = $state(false);

  const source = $derived(sourceOf(editing ? saved : value));

  $effect(() => {
    if (editing && value !== (typed === "" ? saved : typed)) {
      typed = "";
      editing = false;
    }
  });

  function read(): string {
    return source.kind === "none" ? value : typed;
  }

  function write(next: string): void {
    if (source.kind === "none") {
      value = next;
      return;
    }
    typed = next;
    value = next === "" ? saved : next;
  }
</script>

<SecretField {label} {source} bind:value={read, write} bind:editing {hint} {error} {placeholder} />
