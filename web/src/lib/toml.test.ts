import { describe, expect, it } from "vitest";
import { generateProvidersToml } from "./toml";
import type { ProviderKey, SetupWizardState } from "./types";

function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  return {
    userName: "",
    agentName: "assistant",
    timezone: "",
    selectedProviders: ["anthropic"] as ProviderKey[],
    providerConfigs: {
      anthropic: { apiKey: "", model: "", url: "" },
      openai: { apiKey: "", model: "", url: "" },
      gemini: { apiKey: "", model: "", url: "" },
      fireworks: { apiKey: "", model: "", url: "" },
      ollama: { apiKey: "", model: "", url: "" },
    },
    mainProvider: "anthropic",
    roles: {
      observer: { provider: "", url: "", model: "" },
      reflector: { provider: "", url: "", model: "" },
      pulse: { provider: "", url: "", model: "" },
    },
    embeddingModel: { provider: "", model: "" },
    backgroundModels: {
      small: { provider: "", model: "" },
      medium: { provider: "", model: "" },
      large: { provider: "", model: "" },
    },
    mcpServers: [],
    integrations: {
      discordToken: "",
      telegramToken: "",
      teamsAppId: "",
      teamsTenantId: "",
      teamsAppPassword: "",
    },
    secretRefs: {},
    ...overrides,
  };
}

describe("generateProvidersToml", () => {
  it("writes a section for a selected provider even with no typed key", () => {
    const state = wizard({
      selectedProviders: ["anthropic", "openai"] as ProviderKey[],
      providerConfigs: {
        ...wizard().providerConfigs,
        openai: { apiKey: "", model: "", url: "" },
      },
    });
    const toml = generateProvidersToml(state);

    expect(toml).toContain("[providers.openai]");
    expect(toml).toContain('type = "openai"');
    // No api_key line at all — an absent key lets the backend fall back to
    // OPENAI_API_KEY, whereas `api_key = ""` would not.
    const openaiSection = toml.split("[providers.openai]")[1]?.split("[providers.")[0] ?? "";
    expect(openaiSection).not.toContain("api_key");
  });

  it("keeps a typed key's providers.toml entry", () => {
    const state = wizard({
      selectedProviders: ["anthropic"] as ProviderKey[],
      providerConfigs: {
        ...wizard().providerConfigs,
        anthropic: { apiKey: "sk-test-123", model: "", url: "" },
      },
    });
    const toml = generateProvidersToml(state);

    expect(toml).toContain('api_key = "sk-test-123"');
  });

  it("prefers a stored secret reference over the raw typed key", () => {
    const state = wizard({
      selectedProviders: ["anthropic"] as ProviderKey[],
      providerConfigs: {
        ...wizard().providerConfigs,
        anthropic: { apiKey: "sk-test-123", model: "", url: "" },
      },
      secretRefs: { anthropic: "secret:anthropic-ref" },
    });
    const toml = generateProvidersToml(state);

    expect(toml).toContain('api_key = "secret:anthropic-ref"');
    expect(toml).not.toContain("sk-test-123");
  });

  it("preserves a custom base URL even when no key was typed", () => {
    const state = wizard({
      selectedProviders: ["anthropic", "openai"] as ProviderKey[],
      providerConfigs: {
        ...wizard().providerConfigs,
        openai: { apiKey: "", model: "", url: "https://my-vllm.example.com/v1" },
      },
    });
    const toml = generateProvidersToml(state);

    expect(toml).toContain('url = "https://my-vllm.example.com/v1"');
  });

  it("writes an ollama section with no api_key line", () => {
    const state = wizard({ selectedProviders: ["ollama"] as ProviderKey[] });
    const toml = generateProvidersToml(state);

    expect(toml).toContain("[providers.ollama]");
    expect(toml).not.toContain("api_key");
  });
});
