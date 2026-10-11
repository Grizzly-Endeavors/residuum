<script lang="ts">
  import Feed from "../../feed/Feed.svelte";
  import { formatTokenCount } from "../../lib/format-usage";
  import { hub } from "../../lib/hub.svelte";
  import { router } from "../../lib/router.svelte";
  import { openSessionByAddress } from "../../lib/session-address";
  import {
    formatLocalDateTime,
    isStoppableState,
    runIcon,
    runKind,
    runStatus,
    sessionArtifact,
    startedByOwner,
  } from "../../lib/session-format";
  import type { SessionRun } from "../../lib/session-run.svelte";
  import { Banner, Button, Disclosure, Skeleton } from "../../lib/ui";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";
  import RunStatus from "./RunStatus.svelte";
  import SessionMessageBox from "./SessionMessageBox.svelte";

  // A session run in the context panel: what it is and how it's doing, its
  // transcript with live output, Stop, and a message box that reaches the
  // run, or starts the session again once it has finished. The run's state
  // lives in `run`, which the panel host keeps while the frame redraws.

  let { run }: { run: SessionRun } = $props();

  const summary = $derived(run.summary);
  const agentReady = $derived(
    hub.agent(run.agent)?.state === "running" && !hub.isStopping(run.agent),
  );
  const working = $derived(
    summary?.state === "running" || summary?.state === "forking" || summary?.state === "queued",
  );
  const finished = $derived(summary?.state === "completed");
  const artifact = $derived(summary ? sessionArtifact(summary) : null);
  const spawner = $derived(
    summary?.spawner != null && summary.spawner !== "main" ? summary.spawner : null,
  );

  let now = $state(Date.now());
  $effect(() => {
    if (!working && summary?.state !== "idle") return;
    now = Date.now();
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });
</script>

<PanelHeader
  icon={summary ? runIcon(summary.category) : "sessions"}
  kind={summary ? runKind(run.agent, summary) : "Session"}
  title={summary?.purpose || summary?.address || "Session"}
>
  {#snippet meta()}
    {#if summary}<RunStatus status={runStatus(summary, now)} />{/if}
    {#if router.viewedAgent !== run.agent}<span>On {run.agent}</span>{/if}
  {/snippet}
  {#snippet actions()}
    {#if summary && isStoppableState(summary.state)}
      <Button
        variant="quiet"
        size="sm"
        icon="stop"
        loading={run.stopping}
        disabled={!agentReady}
        onclick={() => void run.stop()}>Stop</Button
      >
    {/if}
  {/snippet}
</PanelHeader>

<div class="session-panel">
  {#if summary}
    <div class="session-about">
      <Disclosure summary="Details" tone="quiet">
        <dl class="session-details">
          <dt>Started by</dt>
          <dd>
            {#if artifact}
              The workbench page
              <button
                type="button"
                class="session-link"
                onclick={() => void router.openPlace({ kind: "workbench", artifact })}
                >{artifact}</button
              >
            {:else if spawner}
              The session
              <button
                type="button"
                class="session-link code"
                onclick={() => void openSessionByAddress(run.agent, spawner, null)}
                >{spawner}</button
              >
            {:else if startedByOwner(summary)}
              You{#if summary.source_label === "owner:multitask"}, forking {run.agent}'s
                conversation with /multitask{/if}
            {:else if summary.spawner === "main"}
              {run.agent}'s conversation
            {:else}
              {summary.source_label}
            {/if}
          </dd>
          {#if summary.depth > 1}
            <dt>Depth</dt>
            <dd>{summary.depth} levels below the conversation</dd>
          {/if}
          <dt>Spent</dt>
          <dd>
            {formatTokenCount(summary.usage.input_tokens)} tokens in, {formatTokenCount(
              summary.usage.output_tokens,
            )} out, {summary.usage.tool_calls} tool calls
          </dd>
          {#if summary.episode_id}
            <dt>Remembered as</dt>
            <dd class="code">{summary.episode_id}</dd>
          {/if}
          <dt>Session</dt>
          <dd class="code">{summary.address}</dd>
          <dt>Run</dt>
          <dd class="code">{summary.run_id}</dd>
        </dl>
      </Disclosure>
      {#if summary.interrupted}
        <p class="session-note">
          This run was cut short when Residuum stopped, and was closed out at the next start.
        </p>
      {/if}
      {#if summary.outcome === "failed"}
        <Banner tone="error">
          {summary.error ?? "This run failed."}
          {#if summary.error_details}
            <Disclosure summary="Details" tone="quiet">
              <pre class="session-error-details">{summary.error_details}</pre>
            </Disclosure>
          {/if}
        </Banner>
      {/if}
      {#if summary.overlap}
        <p class="session-note">
          This run started while its previous run, started {formatLocalDateTime(
            summary.overlap.previous_started_at,
          )}, was still going.
        </p>
      {/if}
    </div>
  {/if}

  {#if run.loadError}
    <div class="session-about">
      <Banner tone="error">
        {run.loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void run.load()}>Try again</Button>
        {/snippet}
      </Banner>
    </div>
  {:else if !run.loaded}
    <div class="session-about">
      <Skeleton lines={4} label="Loading the transcript" />
    </div>
  {/if}

  <Feed
    agent={run.agent}
    items={run.items}
    label="Transcript"
    loading={!run.loaded}
    live={working}
    liveTurnId={run.activeTurnId}
    observed={run.observed.get}
  >
    {#snippet empty()}
      <p class="session-empty">No messages in this run yet.</p>
    {/snippet}
    {#snippet tail()}
      <!-- Between turns, or before the panel has seen a turn's frames, the run's own state says it is working. -->
      {#if working && run.activeTurnId === null}
        <p class="session-working">
          <RunStatus status={{ tone: "working", text: "Working" }} />
        </p>
      {/if}
    {/snippet}
  </Feed>

  <SessionMessageBox
    bind:value={run.draft}
    label="Message this session"
    placeholder={finished ? "Message it to start it again…" : "Message this session…"}
    sendLabel="Send to this session"
    sending={run.sending}
    blocked={agentReady ? null : `Start ${run.agent} first.`}
    onsend={() => void run.send()}
  />
</div>

<style>
  .session-panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .session-about {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-10) var(--space-18) 0;
  }

  .session-details {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--space-6) var(--space-16);
    padding: var(--space-6) 0 var(--space-4) var(--space-20);
    font-size: var(--font-size-sm);

    & dt {
      color: var(--color-text-3);
    }

    & dd {
      min-width: 0;
      color: var(--color-text-2);
      overflow-wrap: anywhere;
    }
  }

  .code {
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
  }

  .session-link {
    color: var(--color-vein-bright);
    text-align: start;

    &:hover {
      text-decoration: underline;
    }
  }

  .session-note {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .session-error-details {
    margin-top: var(--space-6);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .session-empty {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
    text-align: center;
  }

  .session-working {
    padding-left: var(--space-2);
  }
</style>
