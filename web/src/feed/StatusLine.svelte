<script lang="ts">
  import { Icon } from "../lib/icons";
  import { Disclosure } from "../lib/ui";

  // A one-line note in a session transcript: a delivery outcome, a command
  // that failed, the run finishing. Technical detail waits behind a disclosure.

  interface Props {
    tone: "info" | "error";
    content: string;
    details?: string;
  }

  let { tone, content, details }: Props = $props();
</script>

<div class="status-line" data-tone={tone} role={tone === "error" ? "alert" : "status"}>
  <Icon name={tone === "error" ? "warning" : "info"} size={14} />
  {#if details}
    <Disclosure summary={content} tone="quiet">
      <pre class="status-details">{details}</pre>
    </Disclosure>
  {:else}
    <span class="status-text">{content}</span>
  {/if}
</div>

<style>
  .status-line {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);

    & > :global(svg) {
      flex: none;
      margin-top: var(--space-4);
    }

    &[data-tone="error"] {
      color: var(--color-err-text);
    }
  }

  .status-text {
    padding-top: var(--space-2);
  }

  .status-details {
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
