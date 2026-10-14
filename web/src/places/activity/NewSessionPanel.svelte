<script lang="ts">
  import { untrack } from "svelte";
  import type { SessionModelSize } from "../../lib/api";
  import { hub } from "../../lib/hub.svelte";
  import { router } from "../../lib/router.svelte";
  import { runIcon } from "../../lib/session-format";
  import { Banner, SegmentedControl } from "../../lib/ui";
  import type { Choice } from "../../lib/ui";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";
  import type { NewSession } from "./new-session.svelte";
  import RunStatus from "./RunStatus.svelte";
  import SessionMessageBox from "./SessionMessageBox.svelte";

  // A session the owner starts by hand, in the context panel: which model it
  // runs on, and the message box whose first message starts it clean. Once
  // the run shows up in the agent's sessions, the session panel takes over.

  let { session }: { session: NewSession } = $props();

  const label = $derived(hub.shownName(session.agent));
  const agentReady = $derived(
    hub.agent(session.agent)?.state === "running" && !hub.isStopping(session.agent),
  );

  const SIZES: readonly Choice<SessionModelSize>[] = [
    { value: "small", label: "Small" },
    { value: "medium", label: "Medium" },
    { value: "large", label: "Large" },
  ];

  $effect(() => {
    const run = session.run;
    if (run === undefined) return;
    untrack(() => {
      void router.replacePanel({ kind: "session", agent: session.agent, runId: run.run_id });
    });
  });
</script>

<PanelHeader icon={runIcon("spawned")} kind="Started by you" title="New session">
  {#snippet meta()}
    <span role="status">
      {#if session.waiting}<RunStatus status={{ tone: "working", text: "Starting" }} />{/if}
    </span>
  {/snippet}
</PanelHeader>

<div class="new-session">
  <div class="new-session-body">
    <p class="new-session-about">
      Starts a clean session on {label}. It doesn't see your conversation with {label}, and its
      replies show here.
    </p>
    <SegmentedControl
      label="Model"
      options={SIZES}
      bind:value={session.model}
      disabled={session.sending || session.waiting}
      hint="Which of {label}'s models it runs on. Larger ones take on harder work and cost more."
    />
    {#if session.error}
      <Banner tone="error">{session.error}</Banner>
    {/if}
  </div>

  <SessionMessageBox
    bind:value={session.prompt}
    label="Task for the new session"
    placeholder="What should the session work on?"
    sendLabel="Start the session"
    sending={session.sending || session.waiting}
    blocked={agentReady ? null : `Start ${label} first.`}
    onsend={() => void session.start()}
  />
</div>

<style>
  .new-session {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .new-session-body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-16);
    min-height: 0;
    padding: var(--space-16) var(--space-18);
    overflow-y: auto;
  }

  .new-session-about {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }
</style>
