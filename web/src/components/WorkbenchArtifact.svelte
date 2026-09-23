<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { artifactUrl, type ArtifactsOrigin } from "../lib/workbench";
  import { WorkbenchBridge } from "../lib/workbench-bridge";
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
  let bridge: WorkbenchBridge | null = null;

  let src = $derived(origin?.ok ? `${artifactUrl(origin.origin, name)}?v=${version}` : null);

  // Reloads keep the frame visible, so an agent editing the artifact reads as
  // the page changing in place rather than flashing out and back.
  function reload() {
    removed = false;
    version += 1;
  }

  // The bridge only talks to the artifacts origin, so it starts once that's known.
  $effect(() => {
    const frameOrigin = origin?.ok ? origin.origin : null;
    if (frameOrigin === null) return;
    const active = new WorkbenchBridge(name, frameOrigin, () => frame?.contentWindow ?? null, {
      origin: window.location.origin,
      fetch: (input, init) => window.fetch(input, init),
      onFrame: (listener) => ws.onFrame(listener),
      onEscape: () => {
        if (full) onSetFull(false);
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

  // Keys typed inside the artifact never reach this page; the SDK forwards Esc.
  function handleKeydown(event: KeyboardEvent) {
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

<svelte:window onkeydown={handleKeydown} />

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
