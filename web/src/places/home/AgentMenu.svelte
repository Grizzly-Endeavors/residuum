<script lang="ts">
  import type { AgentSummary } from "../../lib/hub-types";
  import { router } from "../../lib/router.svelte";
  import {
    IconButton,
    Menu,
    MenuItem,
    MenuSeparator,
    StatusDot,
    type StatusDotState,
  } from "../../lib/ui";
  import { lifecycleApplies, type LifecycleAction } from "../../lib/agent-lifecycle";
  import { agentActions } from "./agent-actions.svelte";
  import { STATE_WORDS } from "./home-model";

  // A board row's "…" menu: open the agent's chat, start, stop or restart it,
  // Start automatically, its settings, and delete it. The heading names the
  // agent's state, which is why an action it can't take is unavailable.

  interface Props {
    agent: AgentSummary;
    /** The state the row shows, stopping included. */
    state: StatusDotState;
    working: boolean;
  }

  let { agent, state, working }: Props = $props();

  const busy = $derived(agentActions.pendingOf(agent.name));

  /** An agent that is stopping takes none of Start, Stop and Restart until it has stopped. */
  function offered(action: LifecycleAction): boolean {
    return state !== "stopping" && lifecycleApplies(action, agent.state) && busy === undefined;
  }
</script>

<Menu label="Manage {agent.name}" align="end">
  {#snippet trigger(props)}
    <IconButton icon="more" label="Manage {agent.name}" {...props} />
  {/snippet}
  {#snippet heading()}
    <StatusDot {state} {working} />
    {agent.name}, {STATE_WORDS[state].toLowerCase()}
  {/snippet}
  <MenuItem
    icon="chat"
    label="Open chat"
    onselect={() => void router.openPlace({ kind: "chat", agent: agent.name })}
  />
  <MenuItem
    icon="play"
    label={busy === "start" ? "Starting…" : "Start"}
    disabled={!offered("start")}
    onselect={() => void agentActions.start(agent.name)}
  />
  <MenuItem
    icon="stop"
    label={busy === "stop" ? "Stopping…" : "Stop"}
    disabled={!offered("stop")}
    onselect={() => void agentActions.stop(agent.name)}
  />
  <MenuItem
    icon="reload"
    label={busy === "restart" ? "Restarting…" : "Restart"}
    disabled={!offered("restart")}
    onselect={() => void agentActions.restart(agent.name)}
  />
  <MenuItem
    icon="clock"
    label="Start automatically"
    checked={agentActions.autostartOf(agent)}
    disabled={busy !== undefined}
    onselect={() => void agentActions.toggleAutostart(agent)}
  />
  <MenuSeparator />
  <MenuItem
    icon="settings"
    label="Settings"
    onselect={() => void router.openSettings({ scope: agent.name, section: null })}
  />
  <MenuItem
    icon="trash"
    label={busy === "delete" ? "Deleting…" : "Delete"}
    tone="danger"
    disabled={busy !== undefined}
    onselect={() => void agentActions.delete(agent)}
  />
</Menu>
