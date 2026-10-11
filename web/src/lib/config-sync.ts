// ── Where config changes made elsewhere are heard ────────────────────
//
// The config write coordinator (`config-coordinator.ts`) tells its
// subscribers about changes it didn't make itself. Two feeds carry them:
//
// - The bound agent's socket sends `workspace_changed` for files under a
//   watched prefix. This registers a watch owner on the agent's `config/`
//   folder, tied to that agent so it applies only while that agent is bound,
//   and follows the viewed agent as it changes.
// - The hub's socket sends `hub_config_reloaded` after each attempt to reload
//   the hub's `config.toml`. One where nothing changed is skipped.
//
// No other agent's files have a feed. The coordinator's re-read before each
// save is the only protection for those.

import { hub } from "./hub.svelte";
import type { HubServerMessage } from "./hub-types";
import { onViewedAgentChange } from "./viewed-agent";
import { ws } from "./ws.svelte";
import {
  agentConfigFile,
  configCoordinator,
  HUB_CONFIG_FILE,
  type AgentConfigFileName,
  type ConfigCoordinator,
} from "./config-coordinator";
import type { WatchHandler, WatchOwner, WatchOwnerOptions } from "./watch-registry";
import { changeMatchesPrefix } from "./workspace-watch";

/** An agent's config folder, in the workspace's paths. */
const CONFIG_DIR = "config";

/** Each watched file, by its path in the workspace. */
const AGENT_FILES: readonly { path: string; name: AgentConfigFileName }[] = [
  { path: `${CONFIG_DIR}/config.toml`, name: "config" },
  { path: `${CONFIG_DIR}/providers.toml`, name: "providers" },
  { path: `${CONFIG_DIR}/mcp.json`, name: "mcp" },
];

/** Where changes come from. The app's sockets, or stand-ins in a test. */
export interface ConfigSyncSources {
  /** The bound agent's watch registry. */
  watches: { register: (handler: WatchHandler, options: WatchOwnerOptions) => WatchOwner };
  /** The agent the socket is bound to now. */
  boundAgent: () => string | null;
  /** Hear of the viewed agent changing. */
  onAgentChange: (listener: (agent: string | null) => void) => () => void;
  /** Hear of every hub frame. */
  onHubFrame: (listener: (frame: HubServerMessage) => void) => () => void;
}

/** Start passing changes from `sources` to `coordinator`. Returns a function that stops. */
export function followConfigChanges(
  coordinator: ConfigCoordinator,
  sources: ConfigSyncSources,
): () => void {
  let owner: WatchOwner | null = null;

  const follow = (agent: string | null): void => {
    owner?.release();
    owner = null;
    if (agent === null) return;
    owner = sources.watches.register(
      {
        changed: (changes) => {
          for (const { path, name } of AGENT_FILES) {
            if (changes.some((change) => changeMatchesPrefix(change.path, path))) {
              void coordinator.externalChange(agentConfigFile(agent, name));
            }
          }
        },
        // The feed lost track of changes, so any of the three may have changed.
        resync: () => {
          void coordinator.externalResync(AGENT_FILES.map((f) => agentConfigFile(agent, f.name)));
        },
      },
      { agent },
    );
    owner.set([CONFIG_DIR]);
  };

  follow(sources.boundAgent());
  const stopAgent = sources.onAgentChange(follow);
  const stopHub = sources.onHubFrame((frame) => {
    if (frame.type !== "hub_config_reloaded") return;
    // A reload that found nothing to apply didn't follow a change. One that
    // failed did: the file on disk changed, even though the hub kept its old config.
    if (frame.ok && !frame.changed) return;
    void coordinator.externalChange(HUB_CONFIG_FILE);
  });

  return () => {
    stopAgent();
    stopHub();
    owner?.release();
    owner = null;
  };
}

/** Wire the app's coordinator to the agent and hub sockets. Call it once, at startup. */
export function startConfigSync(): () => void {
  return followConfigChanges(configCoordinator, {
    watches: ws.watches,
    boundAgent: () => ws.agent,
    onAgentChange: onViewedAgentChange,
    onHubFrame: (listener) => hub.onFrame(listener),
  });
}
