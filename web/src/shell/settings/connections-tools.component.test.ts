import { within, type BoundFunctions, type queries } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hub } from "../../lib/hub.svelte";
import type { AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import type { SectionId } from "../../lib/settings-sections";
import { toast } from "../../lib/toast.svelte";
import {
  fireEvent,
  jsonResponse,
  type FetchHandler,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import { snapshot } from "../../test/hub-frames";
import SettingsModal from "../SettingsModal.svelte";
import { conflictQuestion } from "./changed-on-disk.svelte";

let agent = "";
let server: FakeAgentConfig;
let count = 0;
let answer: FetchHandler;
/** The credentials sent to the secret store, as `[name, value]`. */
let secrets: [string, string][] = [];

const saveBar = (): HTMLElement | null => screen.queryByRole("region", { name: "Unsaved changes" });
const group = (name: string): BoundFunctions<typeof queries> =>
  within(screen.getByRole("region", { name }));

/** The diffs the page sent to the agent's `config.toml`. */
const patches = (): unknown[] =>
  server.requests
    .filter((request) => request.method === "PATCH" && request.url.endsWith("/config/patch"))
    .map((request) => JSON.parse(request.body ?? "{}") as unknown);

function runningAgent(name: string): AgentSummary {
  return {
    name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
  };
}

async function open(section: SectionId, files: { config?: string } = {}): Promise<void> {
  if (files.config !== undefined) server.files.config = files.config;
  render(SettingsModal);
  await router.openSettings({ scope: agent, section });
  await settle();
  await vi.waitFor(() => {
    expect(document.querySelector(".settings-content")).not.toBeNull();
    expect(screen.queryByText("Loading settings")).toBeNull();
  });
}

async function type(input: HTMLElement, value: string): Promise<void> {
  await fireEvent.input(input, { target: { value } });
  await settle();
}

async function click(
  name: string | RegExp,
  inside: BoundFunctions<typeof queries> = screen,
): Promise<void> {
  await fireEvent.click(inside.getByRole("button", { name }));
  await settle();
}

async function save(): Promise<void> {
  await fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await vi.waitFor(() => {
    expect(saveBar()).toBeNull();
  });
}

beforeEach(() => {
  stubWebSocket();
  vi.stubGlobal("matchMedia", (media: string) => ({
    media,
    matches: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `conn-${String(++count)}`;
  secrets = [];
  server = fakeAgentConfig(agent, { hub: 'timezone = "UTC"\n' });
  answer = (url, init) => {
    if (init?.method === "POST" && url === "/api/hub/secrets") {
      const body = JSON.parse(typeof init.body === "string" ? init.body : "{}") as {
        name: string;
        value: string;
      };
      secrets.push([body.name, body.value]);
      return jsonResponse({ reference: `secret:${body.name}` });
    }
    return server.handler(url, init);
  };
  mockFetch(answer);
});

afterEach(async () => {
  conflictQuestion.answer(null);
  hub.handleFrame(snapshot([]));
  await router.replacePlace({ kind: "home" });
  for (const scope of settingsModel.stagedScopes) scope.discard();
});

describe("Connections", () => {
  it("connects Discord: a typed token is stored as a secret and the file gets its reference", async () => {
    await open("connections");
    const discord = group("Discord");

    await type(discord.getByLabelText("Bot token"), "tok-123");
    expect(saveBar()).toHaveTextContent("You have unsaved changes.");
    await save();

    expect(secrets).toEqual([["discord", "tok-123"]]);
    expect(patches()).toEqual([{ discord: { token: "secret:discord" } }]);
    expect(server.files.config).not.toContain("tok-123");
    expect(discord.getByText("Stored securely")).toBeTruthy();
    expect(discord.getByRole("button", { name: "Change Bot token" })).toBeTruthy();
  });

  it("keeps a saved token when Change is cancelled, and when staged edits are discarded", async () => {
    await open("connections", { config: '[discord]\ntoken = "secret:discord"\n' });
    const discord = group("Discord");
    expect(discord.getByText("Stored securely")).toBeTruthy();

    await click("Change Bot token", discord);
    await click("Cancel", discord);
    expect(discord.getByText("Stored securely")).toBeTruthy();
    expect(saveBar()).toBeNull();

    await click("Change Bot token", discord);
    await type(discord.getByLabelText("Bot token"), "another");
    expect(saveBar()).toBeTruthy();
    await click("Discard");
    expect(discord.getByText("Stored securely")).toBeTruthy();
    expect(discord.queryByPlaceholderText(/Paste the token/)).toBeNull();
    expect(saveBar()).toBeNull();
    expect(secrets).toEqual([]);
  });

  it("shows a token read from an environment variable and replaces it", async () => {
    await open("connections", { config: '[telegram]\ntoken = "${TG_TOKEN}"\n' });
    const telegram = group("Telegram");
    expect(telegram.getByText("TG_TOKEN")).toBeTruthy();
    await click("Replace Bot token", telegram);
    await type(telegram.getByLabelText("Bot token"), "123:abc");
    await save();
    expect(secrets).toEqual([["telegram", "123:abc"]]);
  });

  it("disconnects as a staged change that Discard brings back", async () => {
    await open("connections", { config: '[discord]\ntoken = "secret:discord"\n' });
    const discord = group("Discord");

    await click("Disconnect Discord", discord);
    expect(saveBar()).toBeTruthy();
    expect(discord.getByLabelText("Bot token")).toHaveValue("");
    expect(patches()).toEqual([]);

    await click("Discard");
    expect(discord.getByText("Stored securely")).toBeTruthy();

    await click("Disconnect Discord", discord);
    await save();
    expect(patches()).toEqual([{ discord: { token: null } }]);
    expect(discord.queryByRole("button", { name: /Disconnect/ })).toBeNull();
  });

  it("says whether each channel is connected while the agent runs", async () => {
    hub.handleFrame(snapshot([runningAgent(agent)]));
    await open("connections", { config: '[discord]\ntoken = "secret:discord"\n' });

    expect(group("Discord").getByText("Connected")).toBeTruthy();
    expect(group("Telegram").getByText("Not connected")).toBeTruthy();
    expect(screen.queryByText(/to see which of these are connected/)).toBeNull();

    await click("Disconnect Discord", group("Discord"));
    expect(group("Discord").getByText("Disconnects when saved")).toBeTruthy();
  });

  it("asks to start a stopped agent instead of guessing at its connections", async () => {
    await open("connections", { config: '[discord]\ntoken = "secret:discord"\n' });
    expect(screen.getByText(`Start ${agent} to see which of these are connected.`)).toBeTruthy();
    expect(group("Discord").queryByText("Connected")).toBeNull();
    // What the file holds stays editable.
    expect(group("Discord").getByText("Stored securely")).toBeTruthy();
  });

  it("holds numbers as text and leaves the default to an empty box", async () => {
    await open("connections", { config: "[telegram]\ncontext_messages = 12\n" });
    const telegram = group("Telegram");
    const context = telegram.getByLabelText("Earlier messages to read");
    expect(context).toHaveValue(12);

    await type(context, "30");
    await save();
    expect(patches()).toEqual([{ telegram: { context_messages: 30 } }]);
  });

  it("warns while Teams is half filled, and clears when it is complete or empty", async () => {
    await open("connections");
    const teams = group("Microsoft Teams");
    expect(teams.queryByText(/Teams stays off/)).toBeNull();

    await type(teams.getByLabelText("App ID"), "app-1");
    expect(
      teams.getByText(/Teams stays off until the app ID, tenant ID and client secret/),
    ).toBeTruthy();

    await type(teams.getByLabelText("Tenant ID"), "tenant-1");
    await type(teams.getByLabelText("Client secret"), "shh");
    expect(teams.queryByText(/Teams stays off/)).toBeNull();

    await type(teams.getByLabelText("Client secret"), "");
    expect(teams.getByText(/Teams stays off/)).toBeTruthy();
  });

  it("stores the Teams client secret under its own name", async () => {
    await open("connections");
    const teams = group("Microsoft Teams");
    await type(teams.getByLabelText("App ID"), "app-1");
    await type(teams.getByLabelText("Tenant ID"), "tenant-1");
    await type(teams.getByLabelText("Client secret"), "shh");
    await save();
    expect(secrets).toEqual([["teams", "shh"]]);
    expect(patches()).toEqual([
      { teams: { app_id: "app-1", tenant_id: "tenant-1", app_password: "secret:teams" } },
    ]);
  });

  it("shows a warning from a save on the field its key path names", async () => {
    await open("connections");
    mockFetch(async (url, init) => {
      const response = await answer(url, init);
      if (init?.method !== "PATCH" || !url.endsWith("/config/patch")) return response;
      const body = (await response.json()) as Record<string, unknown>;
      return jsonResponse({
        ...body,
        diagnostics: [
          {
            severity: "warning",
            message: "That token looks too short.",
            location: { kind: "path", path: "telegram.token" },
          },
        ],
      });
    });
    await type(group("Telegram").getByLabelText("Bot token"), "nope");
    await save();
    expect(group("Telegram").getByText("That token looks too short.")).toBeTruthy();
    expect(group("Discord").queryByText("That token looks too short.")).toBeNull();
  });
});

describe("Webhooks", () => {
  const withHook =
    '[webhooks.deploys]\nsecret = "secret:webhook_deploys"\nrouting = "agent:code-review"\nformat = "raw"\n';

  it("adds a webhook, previews its address and stores its secret under the webhook's name", async () => {
    await open("connections");
    const hooks = group("Incoming webhooks");
    expect(hooks.getByText("No webhooks yet.")).toBeTruthy();

    await click("Add webhook", hooks);
    expect(hooks.getByText("New webhook")).toBeTruthy();
    expect(hooks.getByLabelText("Name")).toHaveFocus();
    // An unnamed webhook is nothing to save yet.
    expect(saveBar()).toBeNull();

    await type(hooks.getByLabelText("Name"), "deploys");
    expect(hooks.getByText(`/webhook/${agent}/deploys`)).toBeTruthy();
    await type(hooks.getByLabelText("Secret"), "s3cret");
    await type(hooks.getByLabelText("Where it goes"), "agent:code-review");
    await type(hooks.getByLabelText("Fields to read"), "issue.title, issue.body");
    await save();

    expect(secrets).toEqual([["webhook_deploys", "s3cret"]]);
    expect(patches()).toEqual([
      {
        webhooks: {
          deploys: {
            secret: "secret:webhook_deploys",
            routing: "agent:code-review",
            content_fields: ["issue.title", "issue.body"],
          },
        },
      },
    ]);
    expect(hooks.getByText("Stored securely")).toBeTruthy();
  });

  it("hides the fields to read for a raw webhook", async () => {
    await open("connections", { config: withHook });
    const hooks = group("Incoming webhooks");
    expect(hooks.getByLabelText("Name")).toHaveValue("deploys");
    expect(hooks.getByLabelText("Message format")).toHaveValue("raw");
    expect(hooks.queryByLabelText("Fields to read")).toBeNull();
    expect(hooks.getByText("Stored securely")).toBeTruthy();
  });

  it("shows the default format for a webhook that names none, without staging a change", async () => {
    await open("connections", { config: '[webhooks.plain]\nrouting = "inbox"\n' });
    const hooks = group("Incoming webhooks");
    expect(hooks.getByLabelText("Message format")).toHaveValue("parsed");
    expect(hooks.getByLabelText("Fields to read")).toBeTruthy();
    expect(saveBar()).toBeNull();

    await fireEvent.change(hooks.getByLabelText("Message format"), { target: { value: "raw" } });
    await settle();
    expect(hooks.queryByLabelText("Fields to read")).toBeNull();
    await fireEvent.change(hooks.getByLabelText("Message format"), { target: { value: "parsed" } });
    await settle();
    expect(saveBar()).toBeNull();
  });

  it("removes a webhook as a staged change: Undo and Discard bring it back, Save removes it", async () => {
    await open("connections", { config: withHook });
    const hooks = group("Incoming webhooks");

    await click("Remove deploys", hooks);
    expect(hooks.queryByLabelText("Name")).toBeNull();
    expect(saveBar()).toBeTruthy();
    const toasted = [...toast.toasts.values()].at(-1);
    expect(toasted?.message).toBe("Removed deploys.");
    toasted?.action?.onClick();
    await settle();
    expect(hooks.getByLabelText("Name")).toHaveValue("deploys");
    expect(saveBar()).toBeNull();

    await click("Remove deploys", hooks);
    await click("Discard");
    expect(hooks.getByLabelText("Name")).toHaveValue("deploys");

    await click("Remove deploys", hooks);
    await save();
    expect(patches()).toEqual([{ webhooks: { deploys: null } }]);
  });

  it("keeps a renamed webhook's stored secret", async () => {
    await open("connections", { config: withHook });
    const hooks = group("Incoming webhooks");
    await type(hooks.getByLabelText("Name"), "releases");
    expect(hooks.getByText("Stored securely")).toBeTruthy();
    await save();
    expect(secrets).toEqual([]);
    expect(server.files.config).toContain("secret:webhook_deploys");
  });
});

describe("Tools & skills", () => {
  it("adds a skill folder, ignores a repeat and removes one with Undo", async () => {
    await open("tools");
    const skills = group("Skill folders");
    expect(skills.getByText("No extra skill folders.")).toBeTruthy();
    const box = skills.getByLabelText("Skill folder to add");

    await type(box, "~/my-skills");
    await click("Add", skills);
    expect(skills.getByText("~/my-skills")).toBeTruthy();
    expect(box).toHaveValue("");

    await type(box, "~/my-skills");
    await click("Add", skills);
    expect(skills.getByText("That folder is already in the list.")).toBeTruthy();
    await type(box, "");

    await click("Remove ~/my-skills", skills);
    expect(skills.queryByText("~/my-skills")).toBeNull();
    [...toast.toasts.values()].at(-1)?.action?.onClick();
    await settle();
    expect(skills.getByText("~/my-skills")).toBeTruthy();

    await click("Remove ~/my-skills", skills);
    expect(skills.getByText("No extra skill folders.")).toBeTruthy();
    expect(patches()).toEqual([]);
  });

  it("saves the skill and tool folders as lists in their own tables", async () => {
    await open("tools");
    const skills = group("Skill folders");
    await type(skills.getByLabelText("Skill folder to add"), "/opt/skills");
    await fireEvent.submit(
      skills.getByLabelText("Skill folder to add").closest("form") as HTMLFormElement,
    );
    const tools = group("Tool folders");
    await type(tools.getByLabelText("Tool folder to add"), "/opt/bin");
    await click("Add", tools);
    await save();
    expect(patches()).toEqual([
      { skills: { dirs: ["/opt/skills"] }, tools: { path: ["/opt/bin"] } },
    ]);
  });

  it("shows the key for the chosen search service and stores a typed key", async () => {
    await open("tools");
    const search = group("Web search");
    expect(search.queryByLabelText("Brave API key")).toBeNull();

    await fireEvent.change(search.getByLabelText("Search service"), { target: { value: "brave" } });
    await settle();
    await type(search.getByLabelText("Brave API key"), "brave-key");
    expect(search.queryByLabelText("Tavily API key")).toBeNull();
    await save();

    expect(secrets).toEqual([["ws_brave", "brave-key"]]);
    expect(patches()).toEqual([
      { web_search: { backend: "brave", brave: { api_key: "secret:ws_brave" } } },
    ]);
  });

  it("offers a base URL only for Ollama Cloud", async () => {
    await open("tools", { config: '[web_search]\nbackend = "ollama"\n' });
    const search = group("Web search");
    expect(search.getByLabelText("Ollama Cloud address")).toBeTruthy();
    await fireEvent.change(search.getByLabelText("Search service"), {
      target: { value: "tavily" },
    });
    await settle();
    expect(search.queryByLabelText("Ollama Cloud address")).toBeNull();
    expect(search.getByLabelText("Tavily API key")).toBeTruthy();
  });

  it("keeps each provider's own search options behind a disclosure", async () => {
    await open("tools");
    const toggle = screen.getByRole("button", { name: "Search options for each model provider" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    await type(group("Anthropic").getByLabelText("Searches per reply"), "5");
    await type(
      group("Anthropic").getByLabelText("Never search these domains"),
      "reddit.com, x.com",
    );
    await fireEvent.change(group("OpenAI").getByLabelText("Search context size"), {
      target: { value: "high" },
    });
    await type(group("Gemini").getByLabelText("Never search these domains"), "spam.net");
    await save();

    expect(patches()).toEqual([
      {
        web_search: {
          anthropic: { max_uses: 5, blocked_domains: ["reddit.com", "x.com"] },
          openai: { search_context_size: "high" },
          gemini: { exclude_domains: ["spam.net"] },
        },
      },
    ]);
  });
});
