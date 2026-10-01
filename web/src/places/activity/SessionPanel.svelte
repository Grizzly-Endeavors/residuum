<script lang="ts">
  import Feed from "../../feed/Feed.svelte";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { openSessionByAddress } from "../../lib/session-address";
  import {
    formatDuration,
    formatLocalDateTime,
    isStoppableState,
    runIcon,
    runKind,
    runStatus,
    sessionArtifact,
  } from "../../lib/session-format";
  import type { SessionRun } from "../../lib/session-run.svelte";
  import { Banner, Button, Disclosure, IconButton, Skeleton } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";
  import RunStatus from "./RunStatus.svelte";

  // A session run in the context panel: what it is and how it's doing, its
  // transcript with live output, Stop, and a message box that reaches the
  // run, or starts the session again once it has finished. The run's state
  // lives in `run`, which the panel host keeps while the frame redraws.

  let { run }: { run: SessionRun } = $props();

  const uid = $props.id();
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

  function grow(box: HTMLTextAreaElement): void {
    box.style.height = "auto";
    box.style.height = `${String(Math.min(box.scrollHeight, 160))}px`;
  }

  function sendOnEnter(event: KeyboardEvent): void {
    if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
    event.preventDefault();
    if (agentReady) void run.send();
  }
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
    verbose={ws.verbose}
    label="Transcript"
    loading={!run.loaded}
    live={working}
  >
    {#snippet empty()}
      <p class="session-empty">No messages in this run yet.</p>
    {/snippet}
    {#snippet tail()}
      {#if working}
        <p class="session-working">
          <RunStatus
            status={{
              tone: "working",
              text:
                run.turnStartedAt === null
                  ? "Working"
                  : `Working for ${formatDuration(now - run.turnStartedAt)}`,
            }}
          />
        </p>
      {/if}
    {/snippet}
  </Feed>

  <form
    class="session-composer"
    onsubmit={(event) => {
      event.preventDefault();
      void run.send();
    }}
  >
    <div class="session-composer-box">
      <textarea
        aria-label="Message this session"
        rows="1"
        placeholder={finished ? "Message it to start it again…" : "Message this session…"}
        disabled={!agentReady}
        aria-describedby={agentReady ? undefined : `${uid}-why`}
        bind:value={run.draft}
        oninput={(event) => grow(event.currentTarget)}
        onkeydown={sendOnEnter}
      ></textarea>
      <IconButton
        icon="send"
        label="Send to this session"
        type="submit"
        variant="primary"
        size="sm"
        loading={run.sending}
        disabled={!agentReady || run.draft.trim() === ""}
      />
    </div>
    {#if !agentReady}
      <p class="session-composer-why" id="{uid}-why">
        <Icon name="info" size={13} />Start {run.agent} first.
      </p>
    {/if}
  </form>
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
