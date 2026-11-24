const MAX_ARTIFACT_NAME_LEN = 64;

const RESERVED_ARTIFACT_NAME = "api";

export function isValidArtifactName(name: string): boolean {
  return (
    name !== RESERVED_ARTIFACT_NAME &&
    name.length <= MAX_ARTIFACT_NAME_LEN &&
    /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)
  );
}
