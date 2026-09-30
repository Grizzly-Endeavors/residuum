import { isDeepStrictEqual } from "node:util";
import { parse as parseToml } from "smol-toml";
import type { HubServerMessage } from "../src/lib/hub-types";
import type { MockState } from "./state";

type TomlTable = Record<string, unknown>;

/** The config the hub is running on, or an empty one when the file can't be read as TOML. */
function parseOrEmpty(toml: string): TomlTable {
  try {
    return parseToml(toml);
  } catch {
    return {};
  }
}

/** The top-level sections whose settings differ, in name order. */
function changedSections(before: TomlTable, after: TomlTable): string[] {
  return [...new Set([...Object.keys(before), ...Object.keys(after)])]
    .filter((name) => !isDeepStrictEqual(before[name], after[name]))
    .sort();
}

/**
 * The hub's config reload (`reload` in `src/hub/runtime.rs`): it compares the
 * file with the config it is running on, tells hub clients with a notice and
 * a `hub_config_reloaded` frame, and keeps the running config when the file
 * can't be read.
 */
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
      broadcast({ type: "notice", level: "warn", message });
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
    broadcast({ type: "notice", level: "info", message });
    broadcast({ type: "hub_config_reloaded", ok: true, changed: true, message });
  };
}
