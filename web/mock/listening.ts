/**
 * The line the mock prints once its server and its artifacts listener both
 * listen, with the ports they took. The end-to-end suite starts its servers on
 * whatever port is free and reads their ports from this line
 * (`e2e/support/servers.ts`), so its format and the pattern that reads it live
 * here together.
 */

const PREFIX = "  [mock] Listening on port ";
const ARTIFACTS = ", workbench artifacts on port ";
const NO_ARTIFACTS = ", workbench artifacts unavailable";

/** The line for a server on `port` whose artifacts listener took `artifactsPort`, or couldn't listen (`null`). */
export function mockListeningLine(port: number, artifactsPort: number | null): string {
  const artifacts = artifactsPort === null ? NO_ARTIFACTS : `${ARTIFACTS}${String(artifactsPort)}`;
  return `${PREFIX}${String(port)}${artifacts}`;
}

/**
 * What matches the line of a server whose artifacts listener is up, capturing
 * the two ports in the named groups `portGroup` and `artifactsGroup`.
 */
export function mockListeningPattern(portGroup: string, artifactsGroup: string): RegExp {
  const escape = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(
    `${escape(PREFIX.trimStart())}(?<${portGroup}>\\d+)${escape(ARTIFACTS)}(?<${artifactsGroup}>\\d+)`,
  );
}
