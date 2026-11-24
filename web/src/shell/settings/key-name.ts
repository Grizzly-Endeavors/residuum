const KEY_NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;

export const KEY_NAME_RULE =
  "Start with a lowercase letter, then use only lowercase letters, digits and underscores, up to 64 in all.";

export function isKeyName(name: string): boolean {
  return KEY_NAME_PATTERN.test(name);
}
