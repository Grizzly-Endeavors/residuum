<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { workbenchToolPageUrl } from "../lib/api";
  import { notifications } from "../lib/notifications.svelte";
  import { WorkbenchBridge } from "../lib/workbench-bridge";
  import { Icon } from "../lib/icons";

  let {
    name,
    title,
    full,
    onBack,
    onSetFull,
  }: {
    name: string;
    title: string;
    /** The tool fills the window with the Residuum UI hidden. */
    full: boolean;
    onBack: () => void;
    onSetFull: (full: boolean) => void;
  } = $props();

  // Must match TOOL_PAGE_CSP in src/gateway/web/workbench.rs; the browser
  // applies the intersection of the two. Never add allow-same-origin: it
  // would give the tool the web UI's origin and the whole API with it.
  const SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";

  let frame: HTMLIFrameElement | undefined = $state();
  let headingEl: HTMLHeadingElement | undefined = $state();
  let version = $state(0);
  let loaded = $state(false);
  let removed = $state(false);
  let bridge: WorkbenchBridge | null = null;

  let src = $derived(`${workbenchToolPageUrl(name)}?v=${version}`);

  // Reloads keep the frame visible, so an agent editing the tool reads as the
  // page changing in place rather than flashing out and back.
  function reload() {
    removed = false;
    version += 1;
  }

  onMount(() => {
    const active = new WorkbenchBridge(name, () => frame?.contentWindow ?? null, {
      origin: window.location.origin,
      fetch: (input, init) => window.fetch(input, init),
      hasUserActivation: () => navigator.userActivation.isActive,
      isConnected: () => ws.transport.status === "connected",
      sendToAgent: (content) => {
        ws.sendChat(content);
        notifications.surface(
          "notice",
          `"${title}" sent Residuum a message. The reply is in chat.`,
        );
      },
      onFrame: (listener) => ws.onFrame(listener),
      onEscape: () => {
        if (full) onSetFull(false);
      },
    });
    active.start();
    bridge = active;

    const onMessage = (event: MessageEvent) => void active.handleMessage(event.source, event.data);
    window.addEventListener("message", onMessage);

    const stopFrames = ws.onFrame((msg) => {
      if (msg.type === "workbench_tool_updated" && msg.name === name) reload();
      else if (msg.type === "workbench_tool_removed" && msg.name === name) removed = true;
    });

    return () => {
      stopFrames();
      window.removeEventListener("message", onMessage);
      active.stop();
      bridge = null;
    };
  });

  function frameLoaded() {
    bridge?.documentChanged();
    loaded = true;
  }

  // Keys typed inside the tool never reach this page; the SDK forwards Esc.
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

  // Land keyboard and screen reader users at the top of the tool that just opened.
  $effect(() => {
    void name;
    void tick().then(() => headingEl?.focus());
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="workbench-tool" class:full>
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
    <div class="workbench-tool-bar">
      <button type="button" class="session-back" onclick={onBack}>
        <Icon name="back" size={14} />
        Workbench
      </button>
      <div class="workbench-tool-heading">
        <h1 class="workbench-tool-title" tabindex="-1" bind:this={headingEl}>{title}</h1>
        <span class="workbench-slab-path">/workbench/{name}</span>
      </div>
      <button
        type="button"
        class="icon-btn"
        title="Fill the window with this tool (F)"
        aria-label="Full view"
        onclick={() => onSetFull(true)}
      >
        <Icon name="expand" size={16} />
      </button>
      <button
        type="button"
        class="icon-btn"
        title="Reload tool"
        aria-label="Reload tool"
        onclick={reload}
      >
        <Icon name="reload" size={16} />
      </button>
    </div>
  {/if}

  <div class="workbench-stage">
    {#if removed}
      <div class="workbench-empty workbench-removed" role="status">
        <h2 class="workbench-empty-title">This tool was deleted</h2>
        <p>
          Its page is gone from the workbench folder. If the agent is rebuilding it, it reappears
          here when it's written again.
        </p>
        <button type="button" class="btn btn-secondary btn-sm" onclick={onBack}
          >Back to the workbench</button
        >
      </div>
    {/if}
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
  </div>
</div>
