// ── Secret / env-reference detection ──────────────────────────────────
//
// A settings field that holds a credential can be one of three things (see
// `resolve_secret_value` in `src/config/resolve/mod.rs`): a `${ENV_VAR}`
// reference, a `secret:<name>` reference into the encrypted store, or a raw
// literal. Only a literal should ever be sent to `storeSecret` — a
// reference is already resolvable on its own, and round-tripping it through
// the secret store would make the store return the reference text itself
// (unexpanded) on every future lookup.
//
// This is the one place that recognizes the two reference shapes; every
// field that decides whether to store a value, and every field that decides
// how to display one, goes through these two functions rather than
// re-implementing the `startsWith("secret:")` check ad hoc.

/** True for a `secret:<name>` reference into the encrypted secret store. */
export function isSecretReference(value: string): boolean {
  return value.startsWith("secret:");
}

/**
 * The variable name inside a `${VAR_NAME}` token, or `null` if `value`
 * isn't exactly that shape (a non-empty name, nothing else).
 */
export function envReferenceName(value: string): string | null {
  if (!value.startsWith("${") || !value.endsWith("}")) return null;
  const name = value.slice(2, -1);
  return name.length > 0 ? name : null;
}

/** True for a `${ENV_VAR}` reference, resolved from the environment at load time. */
export function isEnvReference(value: string): boolean {
  return envReferenceName(value) !== null;
}

/**
 * True when `value` is already a reference (`secret:<name>` or
 * `${ENV_VAR}`) rather than a raw literal that should be sent to the secret
 * store.
 */
export function isStoredReference(value: string): boolean {
  return isSecretReference(value) || isEnvReference(value);
}
