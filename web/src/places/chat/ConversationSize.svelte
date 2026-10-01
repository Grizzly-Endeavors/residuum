<script lang="ts">
  import { actionRegistry } from "../../lib/action-registry.svelte";
  import { formatApproxWords } from "../../lib/format-usage";
  import { hub } from "../../lib/hub.svelte";
  import { Banner, Button, Disclosure, EmptyState } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";

  // The conversation's size in the context panel: the bound
  // agent's figures in plain words, with the token counts behind a
  // disclosure. A stopped agent shows the figures it last recorded. While a
  // reply runs, what it has written and done so far shows too.

  const agent = $derived(ws.agent ?? "");
  const store = $derived(ws.store);
  // Totals of nothing, as an agent that has never replied has, are no figures yet.
  const usage = $derived(
    store.sessionUsage !== null && store.sessionUsage.input_tokens > 0 ? store.sessionUsage : null,
  );
  const running = $derived(hub.displayStateOf(agent) === "running");
  const summarize = $derived(actionRegistry.all.find((action) => action.id === "chat:observe"));
  const numbers = new Intl.NumberFormat("en-US");

  const times = (count: number): string =>
    count === 1 ? "once" : `${numbers.format(count)} times`;
  const replySoFar = $derived(
    store.turnHasUsage
      ? `About ${formatApproxWords(store.turnOutputTokens)} written, tools used ${times(store.turnToolCalls)}`
      : `Tools used ${times(store.turnToolCalls)}`,
  );
</script>

<PanelHeader icon="memory" title="Conversation size" />
<div class="size">
  {#if usage === null}
    {#if store.usageProblem !== null}
      <Banner tone="error">
        {store.usageProblem}
        {#snippet actions()}
          <Button size="sm" onclick={() => void ws.loadUsage()}>Try again</Button>
        {/snippet}
      </Banner>
    {:else}
      <EmptyState>The figures show once {agent} has replied in this conversation.</EmptyState>
    {/if}
  {:else}
    {#if !running}
      <p class="size-note">
        {agent} isn't running, so these are the figures from when it last replied.
      </p>
    {/if}
    {#if usage.context_tokens !== null}
      <p class="size-lead">
        <strong>About {formatApproxWords(usage.context_tokens)}</strong> go to the model each time
        {agent} replies: its instructions, what it remembers, and the conversation so far.
      </p>
    {/if}
    <dl class="size-figures">
      <div>
        <dt>Read over the conversation</dt>
        <dd>About {formatApproxWords(usage.input_tokens)}</dd>
      </div>
      <div>
        <dt>Written over the conversation</dt>
        <dd>About {formatApproxWords(usage.output_tokens)}</dd>
      </div>
      <div>
        <dt>Tools used</dt>
        <dd>{times(usage.tool_calls)}</dd>
      </div>
      {#if store.isProcessing}
        <div>
          <dt>This reply so far</dt>
          <dd>{replySoFar}</dd>
        </div>
      {/if}
    </dl>
    <p class="size-note">
      When the conversation gets long, {agent} summarizes older messages, so it can keep going without
      forgetting what matters.
    </p>
    {#if summarize !== undefined}
      <div class="size-action">
        <Button
          disabled={summarize.disabled !== undefined}
          onclick={() => void actionRegistry.run(summarize)}>{summarize.label}</Button
        >
        {#if summarize.disabled !== undefined}
          <span class="size-note">{summarize.disabled}.</span>
        {/if}
      </div>
    {/if}
    <Disclosure summary="Token counts" tone="quiet">
      <dl class="size-figures size-tokens">
        <div>
          <dt>Sent at the last reply</dt>
          <dd>
            {usage.context_tokens === null ? "Not known yet" : numbers.format(usage.context_tokens)}
          </dd>
        </div>
        <div>
          <dt>Sent in all</dt>
          <dd>{numbers.format(usage.input_tokens)}</dd>
        </div>
        <div>
          <dt>Written in all</dt>
          <dd>{numbers.format(usage.output_tokens)}</dd>
        </div>
        <div>
          <dt>Tool calls</dt>
          <dd>{numbers.format(usage.tool_calls)}</dd>
        </div>
        {#if store.isProcessing && store.turnHasUsage}
          <div>
            <dt>Written this reply</dt>
            <dd>{numbers.format(store.turnOutputTokens)}</dd>
          </div>
        {/if}
      </dl>
    </Disclosure>
  {/if}
</div>

<style>
  .size {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    gap: var(--space-16);
    padding: var(--space-16) var(--space-18);
    overflow-y: auto;
  }

  .size-lead {
    font-size: var(--font-size-message);
    line-height: var(--line-height-message);

    & strong {
      font-weight: var(--font-weight-semibold);
    }
  }

  .size-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  /* Each figure's label, and its value at the end of the row, or under the label when the row is narrow. */
  .size-figures {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    font-size: var(--font-size-sm);

    & div {
      display: flex;
      flex-wrap: wrap;
      justify-content: space-between;
      gap: var(--space-2) var(--space-16);
    }

    & dt {
      color: var(--color-text-3);
    }

    & dd {
      font-variant-numeric: tabular-nums;
    }
  }

  .size-tokens {
    padding: var(--space-8) 0 0 var(--space-20);

    & dd {
      color: var(--color-text-2);
    }
  }

  .size-action {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-12);
  }
</style>
