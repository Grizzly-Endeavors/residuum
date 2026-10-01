<script lang="ts">
  import { NumberField } from "../../lib/ui";

  // A number of a config form, which holds numbers as text so an empty box
  // can mean "use the default".

  interface Props {
    label: string;
    text?: string;
    hint?: string;
    error?: string;
    /** What the number counts, shown after the box. */
    unit?: string;
    placeholder?: string;
    min?: number;
  }

  let { label, text = $bindable(""), hint, error, unit, placeholder, min }: Props = $props();

  function read(): number | null {
    const parsed = Number(text);
    return text.trim() === "" || !Number.isFinite(parsed) ? null : parsed;
  }

  function write(next: number | null): void {
    text = next === null ? "" : String(next);
  }
</script>

<NumberField {label} bind:value={read, write} {hint} {error} {unit} {placeholder} {min} />
