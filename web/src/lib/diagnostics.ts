// ── Diagnostic formatting ─────────────────────────────────────────────
//
// Shared rendering helpers for `Diagnostic`s returned by
// `POST /api/workspace/validate` and the write/move/save endpoints'
// `diagnostics` field (see `web/src/lib/types.ts`).

import type { Diagnostic, DiagnosticLocation } from "./types";

/** Render a diagnostic's location as a short human string, or `""` when
 * none is known (a semantic problem with no source position). */
export function formatDiagnosticLocation(location: DiagnosticLocation | undefined): string {
  if (!location) return "";
  switch (location.kind) {
    case "line":
      return `line ${location.line}`;
    case "line_column":
      return `line ${location.line}, column ${location.column}`;
    case "path":
      return `at ${location.path}`;
  }
}

/** Render a diagnostic as one line: `"line 14: message"`, or just the
 * message when there's no location. */
export function formatDiagnostic(diagnostic: Diagnostic): string {
  const location = formatDiagnosticLocation(diagnostic.location);
  return location ? `${location}: ${diagnostic.message}` : diagnostic.message;
}
