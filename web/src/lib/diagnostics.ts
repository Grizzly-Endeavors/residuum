import type { Diagnostic, DiagnosticLocation } from "./types";

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

export function formatDiagnostic(diagnostic: Diagnostic): string {
  const location = formatDiagnosticLocation(diagnostic.location);
  return location ? `${location}: ${diagnostic.message}` : diagnostic.message;
}
