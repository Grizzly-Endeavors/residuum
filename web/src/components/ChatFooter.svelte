<script lang="ts">
  import { onMount } from "svelte";
  import { fetchProvidersRaw } from "../lib/api";
  import { parseProvidersToml } from "../lib/settings-toml";
  import { formatTokenCount } from "../lib/format-usage";
  import type { SessionUsageTotals } from "../lib/types";

  let {
    usage = null,
    model = undefined,
    memoryWorking = false,
    subconsciousWorking = false,
  }: {
    /** Cumulative session token totals, or `null` before the first one arrives. */
    usage?: SessionUsageTotals | null;
    /**
     * Model label to show instead of self-fetching the configured main
     * model. Pass `null` (not left undefined) to suppress the model
     * segment entirely — there's no single resolved model string for a
     * background session's run, only a model *tier*, so `SessionView`
     * passes `null` here rather than a misleading main-model label.
     */
    model?: string | null;
    /**
     * Whether the background observer/reflector or subconscious cycle is
     * currently running (see `ws.store.memoryWorking`/`subconsciousWorking`).
     * Neither blocks the next turn — this is purely informational, so a
     * caller with no such state (e.g. `SessionView`) simply omits it.
     */
    memoryWorking?: boolean;
    subconsciousWorking?: boolean;
  } = $props();

  let fetchedModel = $state<string | null>(null);

  // `undefined` (the default) means "self-fetch the main model"; any other
  // value — including `null` — means the caller has already decided what
  // to show and no fetch should happen. Checked once at mount: which mode
  // a given `ChatFooter` instance runs in doesn't change over its lifetime.
  onMount(async () => {
    if (model !== undefined) return;
    try {
      const raw = await fetchProvidersRaw();
      const main = parseProvidersToml(raw).models.main;
      if (!main) return;
      const slashIdx = main.indexOf("/");
      fetchedModel = slashIdx > 0 ? main.slice(slashIdx + 1) : main;
    } catch {
      // Quiet degradation, by design — this is a quiet status line, not
      // worth a toast over. The model segment is simply omitted.
    }
  });

  let modelLabel = $derived(model === undefined ? fetchedModel : model);

  let contextLabel = $derived.by(() => {
    const tokens = usage?.context_tokens;
    return tokens == null ? null : formatTokenCount(tokens);
  });

  // Background, non-blocking housekeeping — never a reason to interrupt or
  // alarm, so it reads as one quiet phrase rather than a status per kind.
  let activityLabel = $derived.by(() => {
    if (memoryWorking && subconsciousWorking) return "updating memory, reviewing turn…";
    if (memoryWorking) return "updating memory…";
    if (subconsciousWorking) return "reviewing turn…";
    return null;
  });
</script>

{#if modelLabel || usage || activityLabel}
  <div class="chat-footer">
    {#if modelLabel}
      <span class="chat-footer-item chat-footer-model">{modelLabel}</span>
    {/if}
    {#if usage}
      {#if modelLabel}<span class="chat-footer-sep">·</span>{/if}
      <span class="chat-footer-item">
        ↑ {formatTokenCount(usage.input_tokens)} ↓ {formatTokenCount(usage.output_tokens)}
      </span>
      {#if contextLabel}
        <span class="chat-footer-sep">·</span>
        <span class="chat-footer-item">{contextLabel} context</span>
      {/if}
    {/if}
    {#if activityLabel}
      {#if modelLabel || usage}<span class="chat-footer-sep">·</span>{/if}
      <span class="chat-footer-item chat-footer-activity">{activityLabel}</span>
    {/if}
  </div>
{/if}

<style>
  .chat-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 14px 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    letter-spacing: 0.01em;
    color: var(--text-muted);
    flex-wrap: wrap;
  }

  .chat-footer-item {
    white-space: nowrap;
  }

  .chat-footer-sep {
    opacity: 0.5;
  }

  .chat-footer-activity {
    font-style: italic;
    animation: activity-breathe 2.4s ease-in-out infinite;
  }

  @keyframes activity-breathe {
    0%,
    100% {
      opacity: 0.55;
    }
    50% {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .chat-footer-activity {
      animation: none;
      opacity: 0.85;
    }
  }
</style>
