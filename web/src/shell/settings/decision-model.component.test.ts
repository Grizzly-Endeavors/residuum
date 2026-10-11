import { afterEach, describe, expect, it } from "vitest";
import { hub } from "../../lib/hub.svelte";
import {
  settingsModel,
  type AgentScopeModel,
  type AllScopeModel,
} from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import AutoModeSection from "./AutoModeSection.svelte";
import SystemOneSection from "./SystemOneSection.svelte";

// The Decision model section (All agents) and the Auto Mode section (an
// agent), drawn on their own over a fake agent's files: what each field
// shows, what an edit stages, and what the connection test sends.

let server: FakeAgentConfig;
let count = 0;
/** The JSON bodies sent to the decision model endpoints, by path. */
let sent: [string, unknown][] = [];
let scopes: (AllScopeModel | AgentScopeModel)[] = [];

function serve(files: { hub?: string; config?: string }): string {
  const agent = `decide-${String(++count)}`;
  sent = [];
  server = fakeAgentConfig(agent, { hub: files.hub ?? 'timezone = "UTC"\n', config: files.config });
  mockFetch((url, init) => {
    if (init?.method === "POST" && url.startsWith("/api/hub/system-one/")) {
      const body = JSON.parse(typeof init.body === "string" ? init.body : "{}") as unknown;
      sent.push([url, body]);
      if (url.endsWith("/models")) {
        return jsonResponse({ models: [{ id: "nimble", description: null }] });
      }
      return jsonResponse({ ok: true, message: "Ollama answered.", answered_by: "nimble" });
    }
    return server.handler(url, init);
  });
  return agent;
}

afterEach(() => {
  for (const scope of scopes) scope.discard();
  scopes = [];
  hub.systemOne = null;
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

async function openAll(hubToml?: string): Promise<AllScopeModel> {
  serve({ hub: hubToml });
  const scope = settingsModel.all();
  scopes.push(scope);
  await scope.reload();
  render(SystemOneSection, { scope, section: "system_one" });
  await settle();
  return scope;
}

async function openAgent(config?: string): Promise<AgentScopeModel> {
  const agent = serve({ config });
  const scope = settingsModel.agent(agent);
  scopes.push(scope);
  await scope.load();
  render(AutoModeSection, { scope, section: "auto_mode" });
  await settle();
  return scope;
}

describe("Decision model", () => {
  it("stages Ollama with a model, and hides the key it doesn't use", async () => {
    const scope = await openAll();
    expect(screen.queryByLabelText("Model")).toBeNull();

    await fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "ollama" } });
    await settle();
    expect(screen.queryByLabelText("API key")).toBeNull();
    await fireEvent.input(screen.getByLabelText("Model"), { target: { value: "nimble" } });
    await settle();

    expect(scope.configFile.patch).toEqual({
      system_one: { provider: "ollama", model: "nimble" },
    });
  });

  it("asks for an address only for Other", async () => {
    await openAll('timezone = "UTC"\n\n[system_one]\nprovider = "typesafe"\n');
    expect(screen.queryByLabelText("Address")).toBeNull();
    expect(screen.getByLabelText("API key")).toBeTruthy();

    await fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "other" } });
    await settle();
    expect(screen.getByLabelText("Address")).toBeTruthy();
  });

  it("tests the values in the form, saved or not, and says what happened", async () => {
    await openAll('timezone = "UTC"\n\n[system_one]\nprovider = "ollama"\nmodel = "nimble"\n');
    await fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    await settle();

    expect(sent).toEqual([
      [
        "/api/hub/system-one/test",
        { provider: "ollama", url: "", model: "nimble", api_key: "", keep_alive: "" },
      ],
    ]);
    expect(screen.getByText("Ollama answered.")).toBeTruthy();
  });

  it("shows an outage the hub reports, in its own words", async () => {
    hub.systemOne = {
      configured: true,
      provider: "Ollama",
      model: "nimble",
      outage: {
        kind: "unreachable",
        message: "Couldn't reach Ollama at http://localhost:11434.",
        since: "2026-10-03T14:00:00Z",
      },
    };
    await openAll('timezone = "UTC"\n\n[system_one]\nprovider = "ollama"\nmodel = "nimble"\n');
    expect(screen.getByText("Not answering")).toBeTruthy();
    expect(screen.getByText("Couldn't reach Ollama at http://localhost:11434.")).toBeTruthy();
  });
});

describe("Auto Mode", () => {
  it("stages turning it on with a rule and an exception", async () => {
    const scope = await openAgent();
    await fireEvent.click(
      screen.getByRole("switch", { name: "Check tool calls against these rules" }),
    );
    await settle();
    expect(screen.getByText("Add a rule below; with none, nothing is checked.")).toBeTruthy();

    await fireEvent.input(screen.getByLabelText("Rule to add"), {
      target: { value: "Pushing to the main branch" },
    });
    await fireEvent.click(screen.getAllByRole("button", { name: "Add" })[0] as HTMLElement);
    await fireEvent.input(screen.getByLabelText("Exception to add"), {
      target: { value: "Pushing to a feature branch" },
    });
    await fireEvent.click(screen.getAllByRole("button", { name: "Add" })[1] as HTMLElement);
    await settle();

    expect(scope.configFile.patch).toEqual({
      auto_mode: {
        enabled: true,
        deny: ["Pushing to the main branch"],
        allow: ["Pushing to a feature branch"],
      },
    });
  });

  it("removes a rule as a staged change", async () => {
    const scope = await openAgent(
      '[auto_mode]\nenabled = true\ndeny = ["Pushing to main", "Deleting files"]\n',
    );
    await fireEvent.click(screen.getByRole("button", { name: "Remove the rule “Deleting files”" }));
    await settle();
    expect(scope.configFile.patch).toEqual({ auto_mode: { deny: ["Pushing to main"] } });
  });

  it("says when no decision model is set up, so calls run unchecked", async () => {
    hub.systemOne = { configured: false, provider: null, model: null, outage: null };
    await openAgent('[auto_mode]\nenabled = true\ndeny = ["Pushing to main"]\n');
    expect(screen.getByText(/none is set up yet/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Set up a decision model" })).toBeTruthy();
  });
});
