import { describe, expect, it } from "vitest";
import { formatDiagnostic, formatDiagnosticLocation } from "./diagnostics";
import type { Diagnostic } from "./types";

describe("formatDiagnosticLocation", () => {
  it("renders a line", () => {
    expect(formatDiagnosticLocation({ kind: "line", line: 3 })).toBe("line 3");
  });

  it("renders a line and column", () => {
    expect(formatDiagnosticLocation({ kind: "line_column", line: 3, column: 5 })).toBe(
      "line 3, column 5",
    );
  });

  it("renders a key path", () => {
    expect(formatDiagnosticLocation({ kind: "path", path: "pulses.morning-check" })).toBe(
      "at pulses.morning-check",
    );
  });

  it("renders empty string when there is no location", () => {
    expect(formatDiagnosticLocation(undefined)).toBe("");
  });
});

describe("formatDiagnostic", () => {
  it("prefixes the message with the location when one is known", () => {
    const diagnostic: Diagnostic = {
      severity: "error",
      message: 'schedule must be a duration like "30m"',
      location: { kind: "line", line: 14 },
    };
    expect(formatDiagnostic(diagnostic)).toBe('line 14: schedule must be a duration like "30m"');
  });

  it("falls back to the bare message with no location", () => {
    const diagnostic: Diagnostic = {
      severity: "error",
      message: "missing timezone",
    };
    expect(formatDiagnostic(diagnostic)).toBe("missing timezone");
  });
});
