const MAX_ARTIFACT_NAME_LEN = 64;

/** The artifacts listener serves the API at `/api`, so no artifact can have that name. */
const RESERVED_ARTIFACT_NAME = "api";

/** The backend's `is_valid_artifact_name`: lowercase letters and digits in single-hyphen-separated words, at most 64 characters, and not `api`. */
export function isValidArtifactName(name: string): boolean {
  return (
    name !== RESERVED_ARTIFACT_NAME &&
    name.length <= MAX_ARTIFACT_NAME_LEN &&
    /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)
  );
}
