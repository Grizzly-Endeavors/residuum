import type { ProviderKey, SetupWizardState } from "../../lib/types";

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

export function providerName(key: string): string {
  return key in PROVIDER_NAMES ? PROVIDER_NAMES[key as ProviderKey] : key;
}

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
