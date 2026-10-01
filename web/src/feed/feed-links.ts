// What a feed's path links open. A feed belongs to one agent, which may not be
// the bound one (a session run on another agent), so every link names its
// agent: on that agent's own places the file opens in the context panel
// beside them, and anywhere else the agent's chat opens with it.

import { router } from "../lib/router.svelte";
import { formatLocation, viewedAgentOf, type Panel } from "../lib/routes";

function filePanel(path: string): Panel {
  return { kind: "file", path };
}

/** The address a path link points at, for opening it in a new tab. */
export function pathHref(agent: string, path: string): string {
  return formatLocation({
    place: { kind: "chat", agent },
    panel: filePanel(path),
    settings: null,
  });
}

/** Open `path`, in `agent`'s workspace (where `team/…` reaches team files), in the context panel. */
export function openPathInPanel(agent: string, path: string): void {
  if (viewedAgentOf(router.place) === agent) void router.openPanel(filePanel(path));
  else void router.openPlace({ kind: "chat", agent }, { panel: filePanel(path) });
}
