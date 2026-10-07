// ── Instance slugs ───────────────────────────────────────────────────
//
// The relay names each of a user's instances by a slug: 1 to 24 characters of
// a-z, 0-9 and inner hyphens. Slugs and display names arrive from the relay
// (or, for a join request, from an arbitrary requester), so nothing acts on
// one, or puts it in a path, before it passes this check. Display names are
// only ever drawn as text.

const INSTANCE_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,22}[a-z0-9])?$/;

/** Whether `value` is a slug the relay could have issued. */
export function isInstanceSlug(value: unknown): value is string {
  return typeof value === "string" && INSTANCE_SLUG.test(value);
}

/** The entries whose slug passes `isInstanceSlug`; the rest are dropped. */
export function withValidSlugs<T extends { slug: string }>(entries: readonly T[]): T[] {
  return entries.filter((entry) => isInstanceSlug(entry.slug));
}

/** What to call an instance: its display name, or its slug when it has none. */
export function instanceName(entry: { slug: string; display_name: string }): string {
  const name = entry.display_name.trim();
  return name === "" ? entry.slug : name;
}
