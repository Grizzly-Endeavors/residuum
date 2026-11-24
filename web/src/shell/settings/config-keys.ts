import type { ConfigFields } from "../../lib/settings-toml";

type KeysHolding<T> = {
  [K in keyof ConfigFields]: ConfigFields[K] extends T ? K : never;
}[keyof ConfigFields];

export type TextKey = KeysHolding<string>;

export type FlagKey = KeysHolding<boolean>;
