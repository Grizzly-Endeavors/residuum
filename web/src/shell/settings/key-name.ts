// The shape of an agent key's or an A2A caller key's name, which the hub
// checks too (`^[a-z][a-z0-9_]{0,63}$`). Checking it here puts the rule next
// to the box instead of in a refused save.

const KEY_NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/;

/** The rule a name has to meet, in the words that explain a refusal. */
export const KEY_NAME_RULE =
  "Start with a lowercase letter, then use only lowercase letters, digits and underscores, up to 64 in all.";

/** Whether `name` (already trimmed) is one the hub accepts. */
export function isKeyName(name: string): boolean {
  return KEY_NAME_PATTERN.test(name);
}
