<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { ws } from "../lib/ws.svelte";
  import { router } from "../lib/router.svelte";
  import { deleteWorkbenchArtifact, fetchWorkbenchInfo, fetchWorkbenchArtifacts } from "../lib/api";
  import { resolveArtifactsOrigin, type ArtifactsOrigin } from "../lib/workbench";
  import { userErrorMessage } from "../lib/errors";
  import { notifications } from "../lib/notifications.svelte";
  import { relativeTime } from "../lib/time";
  import { Icon } from "../lib/icons";
  import type { ArtifactSummary } from "../lib/types";
  import { notifyWithUndo } from "../lib/undo";
  import WorkbenchArtifact from "./WorkbenchArtifact.svelte";

  let { artifact, full, onClose }: { artifact: string | null; full: boolean; onClose: () => void } =
    $props();

  // How long an artifact's seam glows after the agent changes it.
  const CHANGE_GLOW_MS = 2400;

  let artifacts = $state<ArtifactSummary[]>([]);
  let artifactsOrigin = $state<ArtifactsOrigin | null>(null);
  let loadState = $state<"loading" | "ready" | "failed">("loading");
  let loadError = $state("");
  let now = $state(Date.now());
  const justChanged = new SvelteSet<string>();
  const deleting = new SvelteSet<string>();
  let listHeading: HTMLHeadingElement | undefined = $state();

  let currentTitle = $derived(artifacts.find((a) => a.name === artifact)?.title ?? artifact ?? "");

  async function load(): Promise<void> {
    try {
      const [list, info] = await Promise.all([fetchWorkbenchArtifacts(), fetchWorkbenchInfo()]);
      artifacts = list;
      artifactsOrigin = resolveArtifactsOrigin(info, window.location);
      loadState = "ready";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the workbench." });
      loadState = "failed";
    }
  }

  function markChanged(name: string) {
    justChanged.add(name);
    window.setTimeout(() => justChanged.delete(name), CHANGE_GLOW_MS);
  }

  onMount(() => {
    void load();
    const stop = ws.onFrame((frame) => {
      if (frame.type === "artifact_updated") {
        markChanged(frame.name);
        void load();
      } else if (frame.type === "artifact_removed") {
        artifacts = artifacts.filter((a) => a.name !== frame.name);
      }
    });
    const clock = window.setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => {
      stop();
      window.clearInterval(clock);
    };
  });

  // Changes made while disconnected arrive as no frames, so catch up on
  // every reconnect.
  let wasConnected = false;
  $effect(() => {
    const connected = ws.transport.status === "connected";
    if (connected && !wasConnected && loadState !== "loading") untrack(() => void load());
    wasConnected = connected;
  });

  // Returning to the list lands keyboard and screen reader users on its heading.
  $effect(() => {
    if (artifact === null && loadState !== "loading") listHeading?.focus();
  });

  function open(event: MouseEvent, name: string) {
    // Let modified clicks open the artifact in a new tab.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    router.openWorkbench(name);
  }

  async function remove(item: ArtifactSummary) {
    deleting.add(item.name);
    try {
      await deleteWorkbenchArtifact(item.name);
      artifacts = artifacts.filter((a) => a.name !== item.name);
      notifyWithUndo(`Deleted "${item.title}".`, "workspace", `workbench/${item.name}`, load);
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't delete "${item.title}".`,
          notFound: "It was already deleted.",
        }),
      );
      void load();
    } finally {
      deleting.delete(item.name);
    }
  }
</script>

<div class="workbench-view emerges">
  {#if artifact !== null}
    {#key artifact}
      <WorkbenchArtifact
        name={artifact}
        title={currentTitle}
        origin={artifactsOrigin}
        {full}
        onBack={() => router.openWorkbench(null)}
        onSetFull={(next) => router.setWorkbenchFull(next)}
      />
    {/key}
  {:else}
    <div class="settings-header">
      <h1 class="settings-title workbench-title" tabindex="-1" bind:this={listHeading}>
        Workbench
      </h1>
      <button
        class="icon-btn"
        title="Close workbench"
        aria-label="Close workbench"
        onclick={onClose}
      >
        <Icon name="close" size={16} />
      </button>
    </div>

    <div class="workbench-body">
      <div class="workbench-column">
        {#if loadState === "loading"}
          <p class="workbench-note">Loading artifacts…</p>
        {:else if loadState === "failed"}
          <div class="workbench-empty" role="alert">
            <p>{loadError}</p>
            <button class="btn btn-secondary btn-sm" onclick={() => void load()}>Try again</button>
          </div>
        {:else if artifacts.length === 0}
          <div class="workbench-empty">
            <h2 class="workbench-empty-title">Nothing on the bench yet</h2>
            <p>
              Ask Residuum to build you something you can open here: a chart of this month's
              spending, a calculator for a decision you keep revisiting, a dashboard over your
              inbox. Artifacts appear as soon as the agent writes them, and update while it works.
            </p>
          </div>
        {:else}
          <p class="workbench-note">
            Artifacts Residuum built for you. Each one updates live while the agent edits it.
          </p>
          {#if artifactsOrigin !== null && !artifactsOrigin.ok}
            <p class="workbench-unavailable" role="alert">{artifactsOrigin.reason}</p>
          {/if}
          <ul class="workbench-list">
            {#each artifacts as item (item.name)}
              <li class="workbench-slab" class:just-changed={justChanged.has(item.name)}>
                <a
                  class="workbench-slab-link"
                  href="/workbench/{item.name}"
                  onclick={(e) => open(e, item.name)}
                >
                  <span class="workbench-slab-title">{item.title}</span>
                  <span class="workbench-slab-meta">
                    <span class="workbench-slab-path">/workbench/{item.name}</span>
                    <span class="workbench-slab-time">
                      {justChanged.has(item.name)
                        ? "updating now"
                        : `edited ${relativeTime(item.modified_at, now)}`}
                    </span>
                  </span>
                </a>
                <div class="workbench-slab-actions">
                  <button
                    type="button"
                    title="Delete this artifact and its saved data"
                    class="btn btn-sm btn-danger workbench-delete"
                    disabled={deleting.has(item.name)}
                    onclick={() => void remove(item)}
                  >
                    Delete
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  {/if}
</div>
