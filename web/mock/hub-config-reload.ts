import { isDeepStrictEqual } from "node:util";
import { parse as parseToml } from "smol-toml";
import type { HubServerMessage } from "../src/lib/hub-types";
import type { MockState } from "./state";

type TomlTable = Record<string, unknown>;

function parseOrEmpty(toml: string): TomlTable {
  try {
    return parseToml(toml);
  } catch {
    return {};
  }
}

function changedSections(before: TomlTable, after: TomlTable): string[] {
  return [...new Set([...Object.keys(before), ...Object.keys(after)])]
    .filter((name) => !isDeepStrictEqual(before[name], after[name]))
    .sort();
}

export function createHubConfigReloader(
  hubState: MockState,
  broadcast: (frame: HubServerMessage) => void,
): () => void {
  let running = parseOrEmpty(hubState.hubConfigToml);

  return () => {
    let loaded: TomlTable;
    try {
      loaded = parseToml(hubState.hubConfigToml);
    } catch (err) {
      const reason = err instanceof Error ? err.message : String(err);
      const message = `hub config reload failed (keeping current hub config): ${reason}`;
      broadcast({ type: "hub_config_reloaded", ok: false, changed: false, message });
      return;
    }
    if (isDeepStrictEqual(loaded, running)) {
      broadcast({ type: "hub_config_reloaded", ok: true, changed: false, message: null });
      return;
    }
    const changed = changedSections(running, loaded);
    running = loaded;
    const message = `hub configuration reloaded: ${changed.join(", ")}`;
    broadcast({ type: "hub_config_reloaded", ok: true, changed: true, message });
  };
}
