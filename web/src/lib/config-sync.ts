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

const CONFIG_DIR = "config";

const AGENT_FILES: readonly { path: string; name: AgentConfigFileName }[] = [
  { path: `${CONFIG_DIR}/config.toml`, name: "config" },
  { path: `${CONFIG_DIR}/providers.toml`, name: "providers" },
  { path: `${CONFIG_DIR}/mcp.json`, name: "mcp" },
];

export interface ConfigSyncSources {
  watches: { register: (handler: WatchHandler, options: WatchOwnerOptions) => WatchOwner };
  boundAgent: () => string | null;
  onAgentChange: (listener: (agent: string | null) => void) => () => void;
  onHubFrame: (listener: (frame: HubServerMessage) => void) => () => void;
}

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

export function startConfigSync(): () => void {
  return followConfigChanges(configCoordinator, {
    watches: ws.watches,
    boundAgent: () => ws.agent,
    onAgentChange: onViewedAgentChange,
    onHubFrame: (listener) => hub.onFrame(listener),
  });
}
