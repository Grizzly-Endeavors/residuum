// ── Where workbench artifacts are served ──────────────────────────────
//
// Artifacts run on their own origin so they never share the web UI's.
// Through the cloud relay that origin is the one the relay announced;
// anywhere else it's this page's own host on the artifacts listener's port.

import type { WorkbenchInfo } from "./types";

export type ArtifactsOrigin = { ok: true; origin: string } | { ok: false; reason: string };

/** The origin artifacts are served from, as seen from `page` (the UI's location). */
export function resolveArtifactsOrigin(
  info: WorkbenchInfo,
  page: Pick<Location, "origin" | "protocol" | "hostname">,
): ArtifactsOrigin {
  if (info.relay !== null && page.origin === info.relay.ui_origin) {
    return { ok: true, origin: info.relay.artifacts_origin };
  }
  if (info.port !== null) {
    return { ok: true, origin: `${page.protocol}//${page.hostname}:${info.port}` };
  }
  return {
    ok: false,
    reason:
      info.unavailable_reason ??
      "Workbench artifacts aren't being served right now. Restart Residuum, and check its logs if this keeps happening.",
  };
}

/** An artifact's URL on the artifacts origin. The trailing slash keeps its relative URLs inside it. */
export function artifactUrl(origin: string, name: string): string {
  return `${origin}/${encodeURIComponent(name)}/`;
}
