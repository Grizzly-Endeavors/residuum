<script lang="ts">
  import { envReferenceName, isSecretReference } from "../../lib/secrets";
  import { SecretField, type SecretSource } from "../../lib/ui";

  // A credential field of a config form. `value` is the form's own value:
  // empty, a literal the user typed, or a `secret:<name>` or `${VARIABLE}`
  // reference. `saved` is the value on disk. A reference is never shown; the
  // field says where it lives, and Change or Replace opens a box for a new
  // one. Nothing typed keeps the saved value, so Cancel puts it back. A typed
  // literal is exchanged for a stored secret when the scope saves.

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

  /** What was typed over a saved reference. The form holds it too, or the saved value while it is empty. */
  let typed = $state("");
  let editing = $state(false);

  const source = $derived(sourceOf(editing ? saved : value));

  // Discard, a reload and a save that exchanged the typed value for its
  // reference change the form's value from outside the box.
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
