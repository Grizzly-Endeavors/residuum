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
  import { agentLifecycle } from "./agent-lifecycle.svelte";
  import { lifecycleCommands, STATE_WORDS } from "./home-model";

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

  const commands = $derived(lifecycleCommands(state));
  const busy = $derived(agentLifecycle.pendingOf(agent.name));
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
    disabled={!commands.start || busy !== undefined}
    onselect={() => void agentLifecycle.start(agent.name)}
  />
  <MenuItem
    icon="stop"
    label={busy === "stop" ? "Stopping…" : "Stop"}
    disabled={!commands.stop || busy !== undefined}
    onselect={() => void agentLifecycle.stop(agent.name)}
  />
  <MenuItem
    icon="reload"
    label={busy === "restart" ? "Restarting…" : "Restart"}
    disabled={!commands.restart || busy !== undefined}
    onselect={() => void agentLifecycle.restart(agent.name)}
  />
  <MenuItem
    icon="clock"
    label="Start automatically"
    checked={agentLifecycle.autostartOf(agent)}
    disabled={busy !== undefined}
    onselect={() => void agentLifecycle.toggleAutostart(agent)}
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
    onselect={() => void agentLifecycle.delete(agent)}
  />
</Menu>
