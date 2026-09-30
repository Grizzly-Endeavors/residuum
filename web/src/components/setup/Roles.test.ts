import { beforeEach, describe, expect, it } from "vitest";
import { jsonResponse, mockFetch, render, settle } from "../../test/component";
import Roles from "./Roles.svelte";
import type { ProviderKey, SetupWizardState } from "../../lib/types";

function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  return {
    userName: "",
    agentName: "assistant",
    timezone: "",
    selectedProviders: ["anthropic", "openai"] as ProviderKey[],
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

function optionValues(select: HTMLElement): string[] {
  return Array.from(select.querySelectorAll("option")).map((o) => o.value);
}

describe("Assign Models step", () => {
  beforeEach(() => {
    mockFetch((url) => {
      if (url.includes("/providers/models")) {
        return jsonResponse({ models: [{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" }] });
      }
      throw new Error(`unexpected fetch ${url}`);
    });
  });

  it("limits Observer/Reflector/Pulse provider choices to selected providers", async () => {
    render(Roles, { wizardState: wizard(), onNext: () => {}, onBack: () => {} });
    await settle();

    for (const role of ["observer", "reflector", "pulse"]) {
      const select = document.querySelector(`#role-${role}-provider`) as HTMLSelectElement;
      expect(optionValues(select)).toEqual(["anthropic", "openai"]);
    }
  });

  it("limits Background small/medium/large provider choices to selected providers", async () => {
    render(Roles, { wizardState: wizard(), onNext: () => {}, onBack: () => {} });
    await settle();

    for (const tier of ["small", "medium", "large"]) {
      const select = document.querySelector(`#role-bg-${tier}-provider`) as HTMLSelectElement;
      expect(optionValues(select)).toEqual(["anthropic", "openai"]);
    }
  });

  it("falls back to the main provider when a role's stored provider was deselected", async () => {
    const state = wizard({
      selectedProviders: ["anthropic"] as ProviderKey[],
      roles: {
        observer: { provider: "openai", url: "", model: "" },
        reflector: { provider: "", url: "", model: "" },
        pulse: { provider: "", url: "", model: "" },
      },
    });
    render(Roles, { wizardState: state, onNext: () => {}, onBack: () => {} });
    await settle();

    const select = document.querySelector("#role-observer-provider") as HTMLSelectElement;
    expect(select.value).toBe("anthropic");
  });
});
