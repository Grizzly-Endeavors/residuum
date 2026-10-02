// The app's actions in the registry: every place and agent, the bound
// agent's live sessions, every settings section, the chat actions, starting
// and stopping agents, creating one, and help. Places, settings and lifecycle
// for agents other than the bound one are listed only once something is
// typed, so the unsearched palette stays about the agent at hand.

import { actionRegistry, HELP_GROUP, type AppAction } from "../lib/action-registry.svelte";
import { displayState } from "../lib/agent-display-state";
import { lifecycleApplies, type LifecycleAction } from "../lib/agent-lifecycle";
import { CHAT_GROUP, chatActions } from "../lib/chat-actions";
import { hub } from "../lib/hub.svelte";
import type { InstallOffer } from "../lib/install";
import type { IconName } from "../lib/icons";
import { notifications } from "../lib/notifications.svelte";
import { router } from "../lib/router.svelte";
import { HOME, type Place } from "../lib/routes";
import {
  AGENT_SECTIONS,
  ALL_SCOPE,
  ALL_SECTIONS,
  type SectionEntry,
} from "../lib/settings-sections";
import { runIcon, runKind } from "../lib/session-format";
import { ws } from "../lib/ws.svelte";
import { AGENT_PLACES, stateWord } from "./rail-model";
import type { ShellActions } from "./shell-actions";

/** Set while the app can be installed from here; Install app is listed only then. */
export const installOffer = $state<InstallOffer>({ install: null });

/** Whether the Add to Home Screen explanation is open, for browsers that install from the share sheet. */
export const installHelp = $state({ open: false });

/** Whether Show connection status is open. */
export const connectionStatusDialog = $state({ open: false });

const INBOX: Place = { kind: "inbox", agent: null, tab: "active", item: null };

function open(place: Place): () => void {
  return () => void router.openPlace(place);
}

/** Agent names with the bound agent first, then by name. */
function agentNames(): string[] {
  const names = hub.agents.map((agent) => agent.name);
  const bound = ws.agent;
  return bound !== null && names.includes(bound)
    ? [bound, ...names.filter((name) => name !== bound)]
    : names;
}

function places(): AppAction[] {
  const go = (
    id: string,
    label: string,
    hint: string,
    icon: IconName,
    place: Place,
  ): AppAction => ({
    id: `go:${id}`,
    group: "Go to",
    label,
    hint,
    icon,
    run: open(place),
  });
  return [
    go("home", "Home", "Your team", "home", HOME),
    go("inbox", "Inbox", "Every agent", "inbox", INBOX),
    go("workbench", "Workbench", "Team", "grid", { kind: "workbench", artifact: null }),
    go("shared-files", "Shared files", "Team", "folder", { kind: "shared-files" }),
  ];
}

function agents(): AppAction[] {
  return hub.agents.map((agent) => {
    const dot = displayState(agent.state, hub.isStopping(agent.name));
    return {
      id: `agent:${agent.name}`,
      group: "Agents",
      label: agent.name,
      hint: stateWord(dot),
      icon: { dot, working: dot === "running" && hub.activityOf(agent.name).busy },
      terms: agent.role === null ? [] : [agent.role],
      run: open({ kind: "chat", agent: agent.name }),
    };
  });
}

function agentPlaces(): AppAction[] {
  return agentNames().flatMap((name) =>
    AGENT_PLACES.map((entry) => ({
      id: `place:${name}:${entry.kind}`,
      group: `In ${name}`,
      label: entry.label,
      hint: name,
      icon: entry.icon,
      searchOnly: name !== ws.agent,
      run: open({ kind: entry.kind, agent: name }),
    })),
  );
}

/** The bound agent's live sessions, which open in the context panel. */
function sessions(): AppAction[] {
  const agent = ws.agent;
  if (agent === null) return [];
  return ws.sessions.live.map((run) => ({
    id: `session:${agent}:${run.run_id}`,
    group: "Running now",
    label: run.purpose === "" ? "A session" : run.purpose,
    hint: runKind(agent, run),
    icon: runIcon(run.category),
    terms: [run.source_label, agent],
    run: () => {
      const panel = { kind: "session", agent, runId: run.run_id } as const;
      if (router.viewedAgent === agent) void router.openPanel(panel);
      else void router.openPlace({ kind: "activity", agent }, { panel });
    },
  }));
}

function settings(shell: ShellActions): AppAction[] {
  const viewed = router.viewedAgent;
  const section = (scope: string, hint: string, entry: SectionEntry): AppAction => ({
    id: `settings:${scope}/${entry.id}`,
    group: "Settings",
    label: entry.label,
    hint,
    icon: "sliders",
    searchOnly: scope !== ALL_SCOPE && scope !== ws.agent,
    run: () => void router.openSettings({ scope, section: entry.id }),
  });
  return [
    {
      id: "settings:open",
      group: "Settings",
      label: "Open settings",
      hint: viewed === null ? "All agents" : `${viewed} settings`,
      icon: "settings",
      run: shell.openSettings,
    },
    ...agentNames().flatMap((name) =>
      AGENT_SECTIONS.map((entry) => section(name, `${name} settings`, entry)),
    ),
    ...ALL_SECTIONS.map((entry) => section(ALL_SCOPE, "All agents", entry)),
  ];
}

const LIFECYCLE: readonly {
  verb: LifecycleAction;
  label: string;
  doing: string;
  icon: IconName;
}[] = [
  { verb: "start", label: "Start", doing: "Starting", icon: "play" },
  { verb: "stop", label: "Stop", doing: "Stopping", icon: "stop" },
  { verb: "restart", label: "Restart", doing: "Restarting", icon: "reload" },
];

function runLifecycle(verb: LifecycleAction, name: string): Promise<boolean> {
  if (verb === "start") return hub.startAgent(name);
  if (verb === "stop") return hub.stopAgent(name);
  return hub.restartAgent(name);
}

/** Start, Stop and Restart for each agent, where they apply. A failure surfaces from the hub store. */
function lifecycle(): AppAction[] {
  return agentNames().flatMap((name) => {
    const state = hub.displayStateOf(name);
    if (state === null) return [];
    return LIFECYCLE.filter(({ verb }) => lifecycleApplies(verb, state)).map(
      ({ verb, label, doing, icon }) => ({
        id: `lifecycle:${name}:${verb}`,
        group: CHAT_GROUP,
        label: `${label} ${name}`,
        icon,
        searchOnly: name !== ws.agent,
        run: () => {
          notifications.surface("system", `${doing} ${name}.`);
          void runLifecycle(verb, name);
        },
      }),
    );
  });
}

function chat(shell: ShellActions): AppAction[] {
  const agent = ws.agent;
  return chatActions({
    agent,
    state: agent === null ? null : hub.displayStateOf(agent),
    replying: ws.store.activeTurnId !== null,
    hubConnection: hub.transport.status,
    agentConnection: ws.transport.status,
    send: (msg) => {
      ws.send(msg);
    },
    stopReply: () => {
      ws.stop();
    },
    surface: (kind, message) => {
      notifications.surface(kind, message);
    },
    showConversationSize: (name) => {
      const panel = { kind: "size" } as const;
      if (router.viewedAgent === name) void router.openPanel(panel);
      else void router.openPlace({ kind: "chat", agent: name }, { panel });
    },
    askForInboxNote: shell.addInboxNote,
    showConnectionStatus: () => {
      connectionStatusDialog.open = true;
    },
  });
}

function help(shell: ShellActions): AppAction[] {
  const base = { group: HELP_GROUP } as const;
  const items: AppAction[] = [
    {
      ...base,
      id: "help:shortcuts",
      label: "Keyboard shortcuts",
      icon: "help",
      command: "help",
      terms: ["keys"],
      run: shell.openShortcuts,
    },
    {
      ...base,
      id: "help:notifications",
      label: "Recent notifications",
      icon: "info",
      terms: ["errors", "history"],
      run: shell.openNotifications,
    },
    {
      ...base,
      id: "help:feedback",
      label: "Send feedback",
      icon: "spark",
      run: () => {
        shell.openFeedback("feedback");
      },
    },
    {
      ...base,
      id: "help:bug",
      label: "Report a bug",
      icon: "bug",
      run: () => {
        shell.openFeedback("bug");
      },
    },
  ];
  const install = installOffer.install;
  if (install !== null) {
    items.push({ ...base, id: "help:install", label: "Install app", icon: "plus", run: install });
  }
  return items;
}

/** Register the app's actions. Returns a function that removes them. */
export function registerAppActions(shell: ShellActions): () => void {
  const removers = [
    actionRegistry.register("places", places),
    actionRegistry.register("agents", agents),
    actionRegistry.register("agent-places", agentPlaces),
    actionRegistry.register("sessions", sessions),
    actionRegistry.register("settings", () => settings(shell)),
    actionRegistry.register("actions", () => [
      ...chat(shell),
      ...lifecycle(),
      {
        id: "agent:create",
        group: CHAT_GROUP,
        label: "Create an agent",
        icon: "plus",
        terms: ["new agent"],
        run: shell.createAgent,
      },
    ]),
    actionRegistry.register("help", () => help(shell)),
  ];
  return () => {
    for (const remove of removers) remove();
  };
}
