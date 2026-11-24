export function isSecretReference(value: string): boolean {
  return value.startsWith("secret:");
}

export function envReferenceName(value: string): string | null {
  if (!value.startsWith("${") || !value.endsWith("}")) return null;
  const name = value.slice(2, -1);
  return name.length > 0 ? name : null;
}

export function isEnvReference(value: string): boolean {
  return envReferenceName(value) !== null;
}

export function isStoredReference(value: string): boolean {
  return isSecretReference(value) || isEnvReference(value);
}
