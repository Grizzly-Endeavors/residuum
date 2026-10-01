// ── Where workbench artifacts open ────────────────────────────────────
//
// Artifacts run on their own origin so they never share the web UI's.
// Through the cloud relay that origin is the one the relay announced;
// anywhere else it's this page's own host on the artifacts listener's port.

import type { WorkbenchInfo } from "./types";

export type ArtifactsOrigin = { ok: true; origin: string } | { ok: false; reason: string };

/** Why artifacts can't open over HTTPS when no relay origin matches this page. */
export const NO_SECURE_ORIGIN_REASON =
  "This page is served over HTTPS, and Residuum Cloud hasn't reported a workbench address yet. If you're using Residuum Cloud, this clears once it finishes connecting. If you reach Residuum through your own HTTPS proxy, artifacts can't open from it: open Residuum over plain HTTP on your network, or through Residuum Cloud.";

/** Why artifacts can't open when the listener isn't running and the hub gave no reason. */
const NO_LISTENER_REASON =
  "Residuum isn't serving workbench artifacts right now. Restart Residuum, and check its logs if this keeps happening.";

/** The origin artifacts open on, as seen from `page` (the UI's location). */
export function resolveArtifactsOrigin(
  info: WorkbenchInfo,
  page: Pick<Location, "origin" | "protocol" | "hostname">,
): ArtifactsOrigin {
  if (info.relay !== null && page.origin === info.relay.ui_origin) {
    return { ok: true, origin: info.relay.artifacts_origin };
  }
  // The port fallback only ever works when this page itself is plain HTTP: the
  // artifacts listener has no TLS of its own, so `https://host:port` never loads,
  // whether that's Residuum Cloud before the relay announces its origin or a
  // reverse proxy terminating TLS in front of Residuum.
  if (page.protocol === "http:" && info.port !== null) {
    return { ok: true, origin: `${page.protocol}//${page.hostname}:${info.port}` };
  }
  return {
    ok: false,
    reason:
      info.unavailable_reason ??
      (page.protocol === "http:" ? NO_LISTENER_REASON : NO_SECURE_ORIGIN_REASON),
  };
}

/** An artifact's URL on the artifacts origin. The trailing slash keeps its relative URLs inside it. */
export function artifactUrl(origin: string, name: string): string {
  return `${origin}/${encodeURIComponent(name)}/`;
}
