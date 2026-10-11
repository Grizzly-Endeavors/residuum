import type { ConfigFields } from "../../lib/settings-toml";

/** The keys of `ConfigFields` whose form value is of type `T`. */
type KeysHolding<T> = {
  [K in keyof ConfigFields]: ConfigFields[K] extends T ? K : never;
}[keyof ConfigFields];

/** A `config.toml` setting the form keeps as text: a number, a name or a choice. */
export type TextKey = KeysHolding<string>;

/** A `config.toml` setting the form keeps as on or off. */
export type FlagKey = KeysHolding<boolean>;
