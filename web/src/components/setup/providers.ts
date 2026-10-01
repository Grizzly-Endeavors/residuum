import type { ProviderKey, SetupWizardState } from "../../lib/types";

/** The providers the wizard offers, in the order it lists them. */
export const PROVIDER_KEYS: readonly ProviderKey[] = [
  "anthropic",
  "openai",
  "gemini",
  "fireworks",
  "ollama",
];

export const PROVIDER_NAMES: Readonly<Record<ProviderKey, string>> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Google Gemini",
  fireworks: "Fireworks AI",
  ollama: "Ollama",
};

/** A provider's display name, or the key itself for one the wizard doesn't list. */
export function providerName(key: string): string {
  return key in PROVIDER_NAMES ? PROVIDER_NAMES[key as ProviderKey] : key;
}

/**
 * Turns a provider on or off. The last selected provider stays on. Turning one
 * off moves the main role to the first provider left, and clears every other
 * role, background tier and the embedding model that used it (a role with no
 * provider of its own uses the main one), so the Assign models step picks
 * them again from the providers still selected instead of writing a model for
 * a provider with no `providers.toml` section.
 */
export function setProviderSelected(
  state: SetupWizardState,
  key: ProviderKey,
  selected: boolean,
): void {
  const index = state.selectedProviders.indexOf(key);
  if (selected) {
    if (index < 0) state.selectedProviders.push(key);
    return;
  }
  if (index < 0 || state.selectedProviders.length <= 1) return;
  state.selectedProviders.splice(index, 1);
  const wasMain = state.mainProvider === key;
  if (wasMain) {
    state.mainProvider = state.selectedProviders[0] ?? "anthropic";
  }
  for (const choice of [
    ...Object.values(state.roles),
    ...Object.values(state.backgroundModels),
    state.embeddingModel,
  ]) {
    if (choice.provider === key || (wasMain && choice.provider === "")) {
      choice.provider = "";
      choice.model = "";
    }
  }
}
