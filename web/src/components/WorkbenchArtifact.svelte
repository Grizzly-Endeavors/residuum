<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { artifactUrl, type ArtifactsOrigin } from "../lib/workbench";
  import { WorkbenchBridge } from "../lib/workbench-bridge";
  import { isStoppableState, sessionsStartedByArtifact, stateLabel } from "../lib/session-format";
  import type { SessionSummary } from "../lib/types";
  import { Icon } from "../lib/icons";

  let {
    name,
    title,
    origin,
    full,
    onBack,
    onSetFull,
  }: {
    name: string;
    title: string;
    /** Where artifacts are served, or null while that is still loading. */
    origin: ArtifactsOrigin | null;
    /** The artifact fills the window with the Residuum UI hidden. */
    full: boolean;
    onBack: () => void;
    onSetFull: (full: boolean) => void;
  } = $props();

  // The artifact runs on its own origin (the artifacts listener), so
  // allow-same-origin gives it that origin (browser storage, relative
  // files), never the UI's.
  const SANDBOX =
    "allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads";

  let frame: HTMLIFrameElement | undefined = $state();
  let headingEl: HTMLHeadingElement | undefined = $state();
  let version = $state(0);
  let loaded = $state(false);
  let removed = $state(false);
  /** The page was unloaded from Stop page; Restart brings it back. */
  let stopped = $state(false);
  let panelOpen = $state(false);
  /** Mirrors the bridge's own `modelCallsInFlight`, updated by its `onModelCallsChanged`. */
  let modelCallsInFlight = $state(0);
  let bridge: WorkbenchBridge | null = null;

  // The live sessions this artifact started, for the activity panel. Read
  // straight off the sessions store rather than a dedicated
  // `GET /api/sessions?artifact=` fetch: the store already keeps `live`
  // current from session frames app-wide (loaded on every connect and
  // resync, regardless of whether the workbench is open), so filtering it
  // here needs no extra request and can never drift from what the sessions
  // sidebar shows for the same runs.
  let artifactSessions = $derived(sessionsStartedByArtifact(ws.sessions.live, name));
  let hasActivity = $derived(artifactSessions.length > 0 || modelCallsInFlight > 0);

  let src = $derived(
    origin?.ok && !stopped ? `${artifactUrl(origin.origin, name)}?v=${version}` : null,
  );

  // Reloads keep the frame visible, so an agent editing the artifact reads as
  // the page changing in place rather than flashing out and back.
  function reload() {
    removed = false;
    version += 1;
  }

  // The bridge only talks to the artifacts origin, so it starts once that's
  // known, and it's torn down (without being replaced) while the page is
  // stopped.
  $effect(() => {
    const frameOrigin = origin?.ok ? origin.origin : null;
    if (frameOrigin === null || stopped) return;
    const active = new WorkbenchBridge(name, frameOrigin, () => frame?.contentWindow ?? null, {
      origin: window.location.origin,
      fetch: (input, init) => window.fetch(input, init),
      onFrame: (listener) => ws.onFrame(listener),
      onConnectionChange: (listener) => ws.onConnectionChange(listener),
      watchWorkspace: (prefixes) => {
        ws.watchWorkspace(prefixes);
      },
      onEscape: () => {
        if (full) onSetFull(false);
      },
      onModelCallsChanged: (count) => {
        modelCallsInFlight = count;
      },
    });
    active.start();
    bridge = active;

    const onMessage = (event: MessageEvent) =>
      void active.handleMessage(event.source, event.origin, event.data);
    window.addEventListener("message", onMessage);

    return () => {
      window.removeEventListener("message", onMessage);
      active.stop();
      bridge = null;
      modelCallsInFlight = 0;
    };
  });

  onMount(() =>
    ws.onFrame((msg) => {
      if (msg.type === "artifact_updated" && msg.name === name) reload();
      else if (msg.type === "artifact_removed" && msg.name === name) removed = true;
    }),
  );

  function frameLoaded() {
    bridge?.documentChanged();
    loaded = true;
  }

  /**
   * Unloads the artifact's frame: the bridge tears down (aborting its
   * in-flight model calls) and the iframe is removed, ending any loop
   * running in the page. Does not touch the artifact's sessions — they keep
   * running and stay listed here and in the sessions list.
   */
  function stopPage() {
    stopped = true;
    loaded = false;
    panelOpen = false;
  }

  /** Reloads the frame into a fresh document with a new bridge. */
  function restart() {
    stopped = false;
    reload();
  }

  function cancelCalls() {
    bridge?.cancelModelCalls();
  }

  function toggleActivityPanel() {
    panelOpen = !panelOpen;
  }

  function openSession(session: SessionSummary) {
    panelOpen = false;
    ws.sessions.openRun(session.run_id);
  }

  function handleClickOutsidePanel(event: MouseEvent) {
    if (!panelOpen) return;
    const target = event.target as HTMLElement | null;
    if (!target?.closest(".workbench-activity-wrap")) panelOpen = false;
  }

  // Keys typed inside the artifact never reach this page; the SDK forwards Esc.
  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && panelOpen) {
      event.preventDefault();
      panelOpen = false;
      return;
    }
    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable)
      return;
    if (event.key === "Escape" && full) {
      event.preventDefault();
      onSetFull(false);
    } else if (
      (event.key === "f" || event.key === "F") &&
      !event.metaKey &&
      !event.ctrlKey &&
      !event.altKey
    ) {
      event.preventDefault();
      onSetFull(!full);
    }
  }

  // Land keyboard and screen reader users at the top of the artifact that just opened.
  $effect(() => {
    void name;
    void tick().then(() => headingEl?.focus());
  });
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutsidePanel} />

<div class="workbench-artifact" class:full>
  {#if full}
    <button
      type="button"
      class="icon-btn workbench-exit-full"
      title="Show the Residuum UI (Esc)"
      aria-label="Show the Residuum UI"
      onclick={() => onSetFull(false)}
    >
      <Icon name="collapse" size={16} />
    </button>
  {/if}
  {#if !full}
    <div class="workbench-artifact-bar">
      <button type="button" class="session-back" onclick={onBack}>
        <Icon name="back" size={14} />
        Workbench
      </button>
      <div class="workbench-artifact-heading">
        <h1 class="workbench-artifact-title" tabindex="-1" bind:this={headingEl}>{title}</h1>
        <span class="workbench-slab-path">/workbench/{name}</span>
      </div>

      <div class="workbench-activity-wrap">
        <button
          type="button"
          class="workbench-activity-toggle"
          class:active={panelOpen}
          class:has-activity={hasActivity}
          aria-expanded={panelOpen}
          aria-controls="workbench-activity-panel"
          title="Sessions and model calls this artifact started"
          onclick={toggleActivityPanel}
        >
          <Icon name="sessions" size={13} />
          <span>{artifactSessions.length} session{artifactSessions.length === 1 ? "" : "s"}</span>
          {#if modelCallsInFlight > 0}
            <span class="workbench-activity-sep" aria-hidden="true">·</span>
            <span>{modelCallsInFlight} call{modelCallsInFlight === 1 ? "" : "s"}</span>
          {/if}
        </button>
        {#if panelOpen}
          <div class="workbench-activity-panel" id="workbench-activity-panel">
            <div class="workbench-activity-section">
              <h2 class="workbench-activity-heading">Sessions this artifact started</h2>
              {#if artifactSessions.length === 0}
                <p class="workbench-activity-empty">No live sessions.</p>
              {:else}
                <ul class="workbench-activity-sessions">
                  {#each artifactSessions as session (session.run_id)}
                    {@const stopping = ws.sessions.stopping.has(session.address)}
                    <li class="workbench-activity-session">
                      <button
                        type="button"
                        class="workbench-activity-session-open"
                        onclick={() => openSession(session)}
                      >
                        <span class="workbench-activity-session-purpose"
                          >{session.purpose || session.address}</span
                        >
                        <span class="workbench-activity-session-state state-{session.state}"
                          >{stateLabel(session.state)}</span
                        >
                      </button>
                      {#if isStoppableState(session.state)}
                        <button
                          type="button"
                          class="workbench-activity-session-stop"
                          title={stopping ? "Stopping…" : "Stop this session"}
                          aria-label={stopping
                            ? `Stopping ${session.address}`
                            : `Stop ${session.address}`}
                          disabled={stopping}
                          onclick={() => ws.sessions.stop(session.address)}
                        >
                          <Icon name="stop" size={10} />
                        </button>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
            <div class="workbench-activity-section workbench-activity-calls">
              <h2 class="workbench-activity-heading">Model calls</h2>
              <div class="workbench-activity-calls-row">
                <span>{modelCallsInFlight} call{modelCallsInFlight === 1 ? "" : "s"} in flight</span
                >
                <button
                  type="button"
                  class="btn btn-secondary btn-sm"
                  disabled={modelCallsInFlight === 0}
                  onclick={cancelCalls}
                >
                  Cancel calls
                </button>
              </div>
            </div>
          </div>
        {/if}
      </div>

      <button
        type="button"
        class="icon-btn"
        title={stopped
          ? "Page stopped"
          : "Stop the page — unloads it and cancels its model calls, but keeps its sessions running"}
        aria-label="Stop page"
        disabled={stopped}
        onclick={stopPage}
      >
        <Icon name="stop" size={16} />
      </button>
      <button
        type="button"
        class="icon-btn"
        title="Fill the window with this artifact (F)"
        aria-label="Full view"
        onclick={() => onSetFull(true)}
      >
        <Icon name="expand" size={16} />
      </button>
      <button
        type="button"
        class="icon-btn"
        title="Reload artifact"
        aria-label="Reload artifact"
        onclick={reload}
      >
        <Icon name="reload" size={16} />
      </button>
    </div>
  {/if}

  <div class="workbench-stage">
    {#if removed}
      <div class="workbench-empty workbench-removed" role="status">
        <h2 class="workbench-empty-title">This artifact was deleted</h2>
        <p>
          It's gone from the workbench folder. If the agent is rebuilding it, it reappears here when
          it's written again.
        </p>
        <button type="button" class="btn btn-secondary btn-sm" onclick={onBack}
          >Back to the workbench</button
        >
      </div>
    {/if}
    {#if stopped}
      <div class="workbench-empty workbench-removed" role="status">
        <h2 class="workbench-empty-title">Page stopped</h2>
        <p>
          This artifact's page was unloaded, and any model calls it had in flight were cancelled.
          Its sessions keep running — find them above or in the sessions list — and you can restart
          the page whenever you're ready.
        </p>
        <button type="button" class="btn btn-secondary btn-sm" onclick={restart}>Restart</button>
      </div>
    {/if}
    {#if origin !== null && !origin.ok}
      <div class="workbench-empty workbench-removed" role="alert">
        <h2 class="workbench-empty-title">Artifacts can't open right now</h2>
        <p>{origin.reason}</p>
      </div>
    {:else if src !== null}
      <iframe
        bind:this={frame}
        class="workbench-frame"
        class:loaded
        class:hidden={removed}
        {src}
        {title}
        sandbox={SANDBOX}
        allow="clipboard-write; fullscreen"
        referrerpolicy="no-referrer"
        onload={frameLoaded}
      ></iframe>
    {/if}
  </div>
</div>
