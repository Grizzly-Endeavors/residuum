import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsModel, type AgentScopeModel } from "../../lib/settings-model.svelte";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import { chooseOnConflict } from "./changed-on-disk.svelte";
import ModelSection from "./ModelSection.svelte";
import { waitFor } from "../../test/wait";

// The Model section drawn on its own, with a real scope over a fake agent's
// files: the roles it shows, what an edit stages, what a save writes, and
// the field it focuses on arrival.

let agent = "";
let server: FakeAgentConfig;
let count = 0;

async function loaded(files: { providers?: string; config?: string }): Promise<AgentScopeModel> {
  agent = `model-${String(++count)}`;
  server = fakeAgentConfig(agent, files);
  mockFetch(async (url, init) => {
    if (init?.method === "POST" && url === "/api/hub/secrets") {
      const body = typeof init.body === "string" ? init.body : "{}";
      const { name } = JSON.parse(body) as { name: string };
      return jsonResponse({ reference: `secret:${name}` });
    }
    return server.handler(url, init);
  });
  const scope = settingsModel.agent(agent);
  await scope.load();
  return scope;
}

/** The JSON of every PATCH of `file` the page sent, in order. */
const patches = (file: "providers" | "config"): unknown[] =>
  server.requests
    .filter((request) => request.method === "PATCH" && request.url.endsWith(`/${file}/patch`))
    .map((request) => JSON.parse(request.body ?? "null") as unknown);

/** The provider and model selects of a job, by its name. */
function job(name: string): { provider: HTMLSelectElement; model: () => HTMLSelectElement } {
  const group = screen.getByRole("group", { name });
  return {
    provider: group.querySelector("select") as HTMLSelectElement,
    model: () => group.querySelectorAll("select")[1] as HTMLSelectElement,
  };
}

/** A thinking level's option inside `container`. */
function level(container: HTMLElement, label: string): HTMLElement {
  const found = [...container.querySelectorAll<HTMLElement>('[role="radio"]')].find(
    (radio) => radio.textContent.trim() === label,
  );
  if (found === undefined) throw new Error(`no thinking level ${label}`);
  return found;
}

const mainModel = (): HTMLSelectElement =>
  screen
    .getByRole("region", { name: "Main model" })
    .querySelectorAll("select")[1] as HTMLSelectElement;

async function save(scope: AgentScopeModel): Promise<void> {
  await scope.save(chooseOnConflict);
  await settle();
}

beforeEach(() => {
  stubWebSocket();
});

afterEach(() => {
  for (const scope of settingsModel.stagedScopes) scope.discard();
  vi.restoreAllMocks();
});

describe("Model", () => {
  it("shows the main model from its provider's list, and a model the list lacks as such", async () => {
    await loaded({ providers: '[models]\nmain = "anthropic/claude-b"\n' });
    render(ModelSection, { scope: settingsModel.agent(agent), section: "model" });

    expect(await screen.findByRole("option", { name: "Claude A" })).toBeTruthy();
    expect(mainModel().value).toBe("claude-b");
    expect(
      (
        screen
          .getByRole("region", { name: "Main model" })
          .querySelector("select") as HTMLSelectElement
      ).value,
    ).toBe("anthropic");

    const unlisted = await loaded({ providers: '[models]\nmain = "anthropic/claude-z"\n' });
    server.models = [{ id: "claude-a", name: "Claude A" }];
    render(ModelSection, { scope: unlisted, section: "model" });
    expect(await screen.findByRole("option", { name: "claude-z (not in the list)" })).toBeTruthy();
  });

  it("keeps the failover list when the main model or its provider changes", async () => {
    const scope = await loaded({
      providers:
        '[providers.work]\ntype = "openai"\n[models]\nmain = ["anthropic/claude-a", "openai/o3"]\n',
    });
    render(ModelSection, { scope, section: "model" });
    expect(screen.getByText("openai/o3")).toBeTruthy();
    await screen.findByRole("option", { name: "Claude B" });

    await userEvent.setup().selectOptions(mainModel(), "claude-b");
    expect(scope.providersFile.patch).toEqual({
      models: { main: ["anthropic/claude-b", "openai/o3"] },
    });

    const provider = screen
      .getByRole("region", { name: "Main model" })
      .querySelector("select") as HTMLSelectElement;
    await userEvent.setup().selectOptions(provider, "work");
    await save(scope);

    expect(patches("providers")).toEqual([{ models: { main: ["work/gpt-4o", "openai/o3"] } }]);
    expect(scope.models.fallbacks.main).toEqual(["openai/o3"]);
  });

  it("loads the reviewing role's model list when the section opens", async () => {
    await loaded({
      providers: '[models]\nmain = "anthropic/claude-a"\nsubconscious = "anthropic/claude-b"\n',
    });
    render(ModelSection, { scope: settingsModel.agent(agent), section: "model" });

    const reviewing = job("Reviewing replies");
    await waitFor(() => {
      expect([...reviewing.model().options].map((option) => option.text)).toContain("Claude A");
    });
    expect(reviewing.model().value).toBe("claude-b");
    expect(
      screen.getByRole("button", { name: "Use different models for specific jobs" }),
    ).toHaveAttribute("aria-expanded", "true");
  });

  it("saves a job's own model, and its thinking with it", async () => {
    const scope = await loaded({ providers: '[models]\nmain = "anthropic/claude-a"\n' });
    render(ModelSection, { scope, section: "model" });
    const user = userEvent.setup();

    const jobs = screen.getByRole("button", { name: "Use different models for specific jobs" });
    expect(jobs).toHaveAttribute("aria-expanded", "false");
    await user.click(jobs);
    const summarizing = job("Summarizing older messages");
    expect(summarizing.provider.value).toBe("");
    await user.selectOptions(summarizing.provider, "openai");
    await user.click(
      level(screen.getByRole("group", { name: "Summarizing older messages" }), "Low"),
    );
    await save(scope);

    expect(patches("providers")).toEqual([
      { models: { observer: { $inline: { model: "openai/gpt-4o", thinking: "low" } } } },
    ]);
  });

  it("saves thinking and temperature for every model to config.toml", async () => {
    const scope = await loaded({ providers: '[models]\nmain = "anthropic/claude-a"\n' });
    render(ModelSection, { scope, section: "model" });

    const every = screen.getByRole("region", { name: "Every model" });
    await userEvent.setup().click(level(every, "Medium"));
    await userEvent.setup().type(every.querySelector("input") as HTMLInputElement, "0.4");
    await save(scope);

    expect(patches("config")).toEqual([{ temperature: 0.4, thinking: "medium" }]);
    expect(patches("providers")).toEqual([]);
  });

  it("adds a provider with its key stored as a secret, and stages a removal Discard brings back", async () => {
    const scope = await loaded({
      providers: '[providers.old]\ntype = "anthropic"\n[models]\nmain = "old/claude-a"\n',
    });
    render(ModelSection, { scope, section: "model" });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Add a provider" }));
    const form = screen.getByRole("form", { name: "Add a provider" });
    await user.type(form.querySelector("input") as HTMLInputElement, "work");
    await user.selectOptions(form.querySelector("select") as HTMLSelectElement, "openai");
    await user.type(form.querySelector('input[type="password"]') as HTMLInputElement, "sk-test");
    await user.click(screen.getByRole("button", { name: "Add provider" }));
    expect(screen.getByText("work")).toBeTruthy();

    await save(scope);
    expect(patches("providers")).toEqual([
      { providers: { work: { type: "openai", api_key: "secret:work" } } },
    ]);

    await user.click(screen.getByRole("button", { name: "Remove old" }));
    expect(scope.providersFile.patch).toEqual({ providers: { old: null } });
    expect(
      screen.getByText(
        "There's no provider called old. Add it under Providers, or choose another.",
      ),
    ).toBeTruthy();
    scope.discard();
    await settle();
    expect(screen.getByRole("button", { name: "Remove old" })).toBeTruthy();
  });

  it("refuses a provider name a model can't name, or one already taken", async () => {
    await loaded({ providers: '[providers.work]\ntype = "openai"\n' });
    render(ModelSection, { scope: settingsModel.agent(agent), section: "model" });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Add a provider" }));
    const name = screen
      .getByRole("form", { name: "Add a provider" })
      .querySelector("input") as HTMLInputElement;
    await user.type(name, "my/key");
    await user.click(screen.getByRole("button", { name: "Add provider" }));
    expect(screen.getByText("Use only letters, numbers, - and _.")).toBeTruthy();

    await user.clear(name);
    await user.type(name, "work");
    expect(screen.getByText("There's already a provider called work.")).toBeTruthy();
    expect(settingsModel.agent(agent).providers).toHaveLength(1);
  });

  it("flags and focuses the field a check found, once", async () => {
    const scope = await loaded({ providers: '[models]\nmain = "anthropic/gpt-9"\n' });
    const problem = "model 'gpt-9' is not offered by provider 'anthropic'";
    scope.flagProblems("providers", [
      { severity: "error", message: problem, location: { kind: "path", path: "models.main" } },
    ]);
    scope.requestFocus({ kind: "role", role: "main" });
    render(ModelSection, { scope, section: "model" });

    await waitFor(() => {
      expect(document.activeElement).toBe(mainModel());
    });
    expect(mainModel()).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText(problem)).toBeTruthy();
    expect(mainModel().closest("[data-arrived]")).not.toBeNull();
    expect(scope.focusRequest).toBeNull();
  });

  it("opens the jobs and focuses the reviewing role when asked for it", async () => {
    const scope = await loaded({ providers: '[models]\nmain = "anthropic/claude-a"\n' });
    scope.requestFocus({ kind: "role", role: "subconscious" });
    render(ModelSection, { scope, section: "model" });

    await waitFor(() => {
      expect(document.activeElement).toBe(job("Reviewing replies").provider);
    });
    expect(
      screen.getByRole("button", { name: "Use different models for specific jobs" }),
    ).toHaveAttribute("aria-expanded", "true");
  });
});
