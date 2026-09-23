<script lang="ts">
  import { tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { router } from "../lib/router.svelte";
  import { Icon } from "../lib/icons";
  import type { SessionView } from "../lib/sessions.svelte";
  import {
    categoryDescription,
    isStoppableState,
    runDuration,
    sessionArtifact,
    sessionSourceText,
    stateLabel,
  } from "../lib/session-format";
  import CategoryBadge from "./CategoryBadge.svelte";
  import SessionFeed from "./SessionFeed.svelte";

  let { view, onBack }: { view: SessionView; onBack: () => void } = $props();

  let headingEl: HTMLHeadingElement | undefined = $state();
  let textarea: HTMLTextAreaElement | undefined = $state();
  let draft = $state("");

  let summary = $derived(view.summary);
  let connected = $derived(ws.transport.status === "connected");
  let finished = $derived(summary?.state === "completed");
  let working = $derived(summary?.state === "running" || summary?.state === "forking");
  let canStop = $derived(summary ? isStoppableState(summary.state) : false);
  let outcome = $derived(ws.sessions.outcomes.get(view.runId));
  let stateText = $derived.by(() => {
    if (!summary) return "";
    if (!finished) return stateLabel(summary.state);
    if (outcome?.status === "failed") return "failed";
    if (outcome?.status === "cancelled") return "stopped";
    return summary.interrupted ? "interrupted" : "finished";
  });
  let spawnerIsSession = $derived(summary?.spawner != null && summary.spawner !== "main");
  let artifact = $derived(summary ? sessionArtifact(summary) : null);

  // Move focus to the heading whenever a different session is opened, so
  // screen reader and keyboard users land at the top of what just replaced
  // the chat. Following the same session into a new run keeps focus where it
  // is (usually the message box).
  $effect(() => {
    void view;
    void tick().then(() => headingEl?.focus());
  });

  function autoResize() {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 160)}px`;
  }

  function send() {
    const text = draft.trim();
    if (!text || !summary || !connected) return;
    ws.sessions.sendMessage(summary.address, text);
    draft = "";
    void tick().then(autoResize);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send();
    }
  }

  function openSpawner() {
    if (summary?.spawner) void ws.sessions.openAddress(summary.spawner, null);
  }
</script>

<section class="session-view" aria-labelledby="session-view-title">
  <header class="session-view-head">
    <button type="button" class="session-back" onclick={onBack}>
      <Icon name="back" size={14} />
      Main chat
    </button>
    {#if canStop && summary}
      <button
        type="button"
        class="session-stop"
        disabled={view.stopRequested || !connected}
        onclick={() => summary && ws.sessions.stop(summary.address)}
      >
        <Icon name="stop" size={12} />
        {view.stopRequested ? "Stopping…" : "Stop session"}
      </button>
    {/if}
  </header>

  <div class="session-view-intro">
    <h2 id="session-view-title" class="session-view-title" tabindex="-1" bind:this={headingEl}>
      {summary?.address ?? "Session"}
    </h2>
    {#if summary}
      <div class="session-view-tags">
        <CategoryBadge category={summary.category} />
        <span class="session-view-source">{sessionSourceText(summary)}</span>
        <span
          class="session-view-state state-{summary.state}"
          class:failed={outcome?.status === "failed"}
        >
          {stateText}
        </span>
        <span class="session-view-duration">{runDuration(summary, ws.sessions.now)}</span>
      </div>
      {#if summary.purpose}
        <p class="session-view-purpose">{summary.purpose}</p>
      {/if}
      <dl class="session-view-meta">
        <div>
          <dt>Kind</dt>
          <dd>{categoryDescription(summary.category)}</dd>
        </div>
        {#if artifact}
          <div>
            <dt>Started by</dt>
            <dd>
              the workbench artifact
              <button
                type="button"
                class="session-meta-link"
                onclick={() => router.openWorkbench(artifact)}
              >
                {artifact}
              </button>
            </dd>
          </div>
        {/if}
        {#if summary.spawner}
          <div>
            <dt>Started by</dt>
            <dd>
              {#if spawnerIsSession}
                <button type="button" class="session-meta-link" onclick={openSpawner}>
                  {summary.spawner}
                </button>
              {:else}
                main agent
              {/if}
            </dd>
          </div>
        {/if}
        {#if summary.depth > 1}
          <div>
            <dt>Nesting</dt>
            <dd>{summary.depth} levels below main</dd>
          </div>
        {/if}
        {#if summary.episode_id}
          <div>
            <dt>Remembered as</dt>
            <dd>{summary.episode_id}</dd>
          </div>
        {/if}
        <div>
          <dt>Run</dt>
          <dd>{summary.run_id}</dd>
        </div>
      </dl>
      {#if summary.interrupted}
        <p class="session-view-note">
          This run was cut short when Residuum stopped, and was closed out at the next start.
        </p>
      {/if}
    {/if}
  </div>

  <SessionFeed
    items={view.items}
    verbose={ws.verbose}
    {working}
    loading={view.loading}
    loadError={view.loadError}
    onRetry={() => void view.load()}
  />

  {#if summary}
    <div class="chat-input-area session-input-area">
      <div class="chat-input-wrap">
        <div class="chat-input-row">
          <label class="visually-hidden" for="session-input">Message {summary.address}</label>
          <textarea
            id="session-input"
            class="chat-input"
            rows="1"
            placeholder={finished
              ? "Message this session to start it again…"
              : "Message this session…"}
            disabled={!connected}
            bind:this={textarea}
            bind:value={draft}
            oninput={autoResize}
            onkeydown={handleKeydown}
          ></textarea>
          <button
            type="button"
            class="send-btn"
            disabled={!connected || !draft.trim()}
            onclick={send}
          >
            Send
          </button>
        </div>
      </div>
    </div>
  {/if}
</section>
