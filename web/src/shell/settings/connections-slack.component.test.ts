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
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
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
  agent = `slack-${String(++count)}`;
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

describe("Slack", () => {
  it("connects Slack: typed tokens are stored as secrets and the file gets their references", async () => {
    await open("connections");
    const slack = group("Slack");

    await type(slack.getByLabelText("Bot token"), "xoxb-123");
    await type(slack.getByLabelText("App token"), "xapp-456");
    expect(saveBar()).toHaveTextContent("You have unsaved changes.");
    await save();

    expect(secrets).toEqual([
      ["slack_bot_token", "xoxb-123"],
      ["slack_app_token", "xapp-456"],
    ]);
    expect(patches()).toEqual([
      { slack: { bot_token: "secret:slack_bot_token", app_token: "secret:slack_app_token" } },
    ]);
    expect(server.files.config).not.toContain("xoxb-123");
    expect(slack.getAllByText("Stored securely").length).toBe(2);
    expect(slack.getByRole("button", { name: "Change Bot token" })).toBeTruthy();
    expect(slack.getByRole("button", { name: "Change App token" })).toBeTruthy();
  });

  it("shows a token read from an environment variable and replaces it", async () => {
    await open("connections", {
      config: '[slack]\nbot_token = "${SLACK_BOT_TOKEN}"\napp_token = "secret:slack_app_token"\n',
    });
    const slack = group("Slack");
    expect(slack.getByText("SLACK_BOT_TOKEN")).toBeTruthy();
    expect(slack.getByText("Stored securely")).toBeTruthy();

    await click("Change App token", slack);
    await type(slack.getByLabelText("App token"), "xapp-new");
    await save();
    expect(secrets).toEqual([["slack_app_token", "xapp-new"]]);
  });

  it("lets others talk to the agent with a toggle", async () => {
    await open("connections", {
      config: '[slack]\nbot_token = "xoxb-1"\napp_token = "xapp-1"\n',
    });
    const slack = group("Slack");
    await fireEvent.click(slack.getByRole("switch", { name: "Let others talk to this agent" }));
    await settle();
    await save();
    expect(patches()).toEqual([{ slack: { respond_to_others: true } }]);
  });

  it("warns while Slack is half filled, and clears when it is complete or empty", async () => {
    await open("connections");
    const slack = group("Slack");
    expect(slack.queryByText(/Slack stays off/)).toBeNull();

    await type(slack.getByLabelText("Bot token"), "xoxb-1");
    expect(slack.getByText(/Slack stays off until the bot token and app token/)).toBeTruthy();

    await type(slack.getByLabelText("App token"), "xapp-1");
    expect(slack.queryByText(/Slack stays off/)).toBeNull();

    await type(slack.getByLabelText("App token"), "");
    expect(slack.getByText(/Slack stays off/)).toBeTruthy();
  });

  it("disconnects as a staged change that saving clears from the file", async () => {
    await open("connections", {
      config:
        '[slack]\nbot_token = "secret:slack_bot_token"\napp_token = "secret:slack_app_token"\n',
    });
    const slack = group("Slack");

    await click("Disconnect Slack", slack);
    expect(saveBar()).toBeTruthy();
    expect(slack.getByLabelText("Bot token")).toHaveValue("");
    expect(slack.getByLabelText("App token")).toHaveValue("");
    expect(patches()).toEqual([]);

    await save();
    expect(patches()).toEqual([{ slack: { bot_token: null, app_token: null } }]);
    expect(slack.queryByRole("button", { name: /Disconnect/ })).toBeNull();
  });

  it("says whether Slack is connected while the agent runs", async () => {
    hub.handleFrame(snapshot([runningAgent(agent)]));
    await open("connections", {
      config: '[slack]\nbot_token = "xoxb-1"\napp_token = "xapp-1"\n',
    });

    expect(group("Slack").getByText("Connected")).toBeTruthy();
    expect(group("Discord").getByText("Not connected")).toBeTruthy();

    await click("Disconnect Slack", group("Slack"));
    expect(group("Slack").getByText("Disconnects when saved")).toBeTruthy();
  });

  it("asks to start a stopped agent instead of guessing at its connections", async () => {
    await open("connections", {
      config: '[slack]\nbot_token = "xoxb-1"\napp_token = "xapp-1"\n',
    });
    expect(screen.getByText(`Start ${agent} to see which of these are connected.`)).toBeTruthy();
    expect(group("Slack").queryByText("Connected")).toBeNull();
    // What the file holds stays editable.
    expect(group("Slack").getByLabelText("Bot token")).toHaveValue("xoxb-1");
    expect(group("Slack").getByLabelText("App token")).toHaveValue("xapp-1");
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
            location: { kind: "path", path: "slack.bot_token" },
          },
        ],
      });
    });
    await type(group("Slack").getByLabelText("Bot token"), "nope");
    await save();
    expect(group("Slack").getByText("That token looks too short.")).toBeTruthy();
    expect(group("Telegram").queryByText("That token looks too short.")).toBeNull();
  });
});
