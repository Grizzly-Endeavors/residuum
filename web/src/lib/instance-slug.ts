const INSTANCE_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,22}[a-z0-9])?$/;

export function isInstanceSlug(value: unknown): value is string {
  return typeof value === "string" && INSTANCE_SLUG.test(value);
}

export function withValidSlugs<T extends { slug: string }>(entries: readonly T[]): T[] {
  return entries.filter((entry) => isInstanceSlug(entry.slug));
}

export function instanceName(entry: { slug: string; display_name: string }): string {
  const name = entry.display_name.trim();
  return name === "" ? entry.slug : name;
}
