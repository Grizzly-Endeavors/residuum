const PREFIX = "  [mock] Listening on port ";
const ARTIFACTS = ", workbench artifacts on port ";
const NO_ARTIFACTS = ", workbench artifacts unavailable";

export function mockListeningLine(port: number, artifactsPort: number | null): string {
  const artifacts = artifactsPort === null ? NO_ARTIFACTS : `${ARTIFACTS}${String(artifactsPort)}`;
  return `${PREFIX}${String(port)}${artifacts}`;
}

export function mockListeningPattern(portGroup: string, artifactsGroup: string): RegExp {
  const escape = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(
    `${escape(PREFIX.trimStart())}(?<${portGroup}>\\d+)${escape(ARTIFACTS)}(?<${artifactsGroup}>\\d+)`,
  );
}
