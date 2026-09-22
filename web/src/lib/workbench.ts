// ── Where workbench tools are served ─────────────────────────────────
//
// Tools run on their own origin so they never share the web UI's. Through
// the cloud relay that origin is the one the relay announced; anywhere else
// it's this page's own host on the tools listener's port.

import type { WorkbenchInfo } from "./types";

export type ToolsOrigin = { ok: true; origin: string } | { ok: false; reason: string };

/** The origin tools are served from, as seen from `page` (the UI's location). */
export function resolveToolsOrigin(
  info: WorkbenchInfo,
  page: Pick<Location, "origin" | "protocol" | "hostname">,
): ToolsOrigin {
  if (info.relay !== null && page.origin === info.relay.ui_origin) {
    return { ok: true, origin: info.relay.tools_origin };
  }
  if (info.port !== null) {
    return { ok: true, origin: `${page.protocol}//${page.hostname}:${info.port}` };
  }
  return {
    ok: false,
    reason:
      info.unavailable_reason ??
      "Workbench tools aren't being served right now. Restart Residuum, and check its logs if this keeps happening.",
  };
}

/** A tool's URL on the tools origin. The trailing slash keeps its relative URLs inside it. */
export function toolUrl(origin: string, name: string): string {
  return `${origin}/${encodeURIComponent(name)}/`;
}
