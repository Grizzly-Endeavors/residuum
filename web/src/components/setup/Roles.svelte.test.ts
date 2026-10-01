import { within } from "@testing-library/svelte";
import { beforeEach, describe, expect, it } from "vitest";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import Roles from "./Roles.svelte";
import type { ProviderKey, SetupWizardState } from "../../lib/types";

// Reactive, as the wizard's own state is, so the step redraws after a change.
function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  const state = $state<SetupWizardState>({
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
  });
  return state;
}

function roleField(role: string, field: "Provider" | "Model"): HTMLSelectElement {
  return within(screen.getByRole("group", { name: role })).getByLabelText(field);
}

function optionValues(select: HTMLSelectElement): string[] {
  return Array.from(select.options).map((o) => o.value);
}

describe("Assign models step", () => {
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

    for (const role of ["Observer", "Reflector", "Pulse"]) {
      expect(optionValues(roleField(role, "Provider"))).toEqual(["anthropic", "openai"]);
    }
  });

  it("limits Background small/medium/large provider choices to selected providers", async () => {
    render(Roles, { wizardState: wizard(), onNext: () => {}, onBack: () => {} });
    await settle();

    for (const tier of ["Small", "Medium", "Large"]) {
      expect(optionValues(roleField(tier, "Provider"))).toEqual(["anthropic", "openai"]);
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

    expect(roleField("Observer", "Provider")).toHaveValue("anthropic");
  });

  it("picks the provider's default model for a role left unset", async () => {
    const state = wizard();
    render(Roles, { wizardState: state, onNext: () => {}, onBack: () => {} });
    await settle();

    expect(roleField("Pulse", "Model")).toHaveValue("claude-sonnet-4-6");
    expect(state.roles.pulse?.model).toBe("claude-sonnet-4-6");
  });

  it("shows a model typed under Other again when the step opens", async () => {
    const state = wizard({
      roles: {
        observer: { provider: "", url: "", model: "my-local-model" },
        reflector: { provider: "", url: "", model: "" },
        pulse: { provider: "", url: "", model: "" },
      },
    });
    render(Roles, { wizardState: state, onNext: () => {}, onBack: () => {} });
    await settle();

    const observer = within(screen.getByRole("group", { name: "Observer" }));
    expect(observer.getByLabelText("Model")).toHaveValue("__other__");
    expect(observer.getByLabelText("Model ID")).toHaveValue("my-local-model");
    expect(state.roles.observer?.model).toBe("my-local-model");
  });
});
