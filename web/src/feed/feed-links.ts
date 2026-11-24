import { router } from "../lib/router.svelte";
import { formatLocation, viewedAgentOf, type Panel } from "../lib/routes";

function filePanel(path: string): Panel {
  return { kind: "file", path, agent: null };
}

export function pathHref(agent: string, path: string): string {
  return formatLocation({
    place: { kind: "chat", agent },
    panel: filePanel(path),
    settings: null,
  });
}

export function openPathInPanel(agent: string, path: string): void {
  if (viewedAgentOf(router.place) === agent) void router.openPanel(filePanel(path));
  else void router.openPlace({ kind: "chat", agent }, { panel: filePanel(path) });
}

export function openAgentFileInPanel(agent: string, path: string): void {
  void router.openPanel({ kind: "file", path, agent });
}

export function teamPathHref(path: string): string {
  return formatLocation({
    place: { kind: "shared-files" },
    panel: filePanel(path),
    settings: null,
  });
}

export function openTeamPathInPanel(path: string): void {
  if (router.place.kind === "shared-files") void router.openPanel(filePanel(path));
  else void router.openPlace({ kind: "shared-files" }, { panel: filePanel(path) });
}
