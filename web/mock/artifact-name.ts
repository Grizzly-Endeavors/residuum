const MAX_ARTIFACT_NAME_LEN = 64;

/** The backend's `is_valid_artifact_name`: lowercase letters and digits in single-hyphen-separated words, at most 64 characters. */
export function isValidArtifactName(name: string): boolean {
  return name.length <= MAX_ARTIFACT_NAME_LEN && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name);
}
