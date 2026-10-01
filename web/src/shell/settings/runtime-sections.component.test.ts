import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { settingsModel, type AgentScopeModel } from "../../lib/settings-model.svelte";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import { chooseOnConflict } from "./changed-on-disk.svelte";
import Memory from "./Memory.svelte";
import Runtime from "./Runtime.svelte";
import Schedule from "./Schedule.svelte";

// The Memory, Schedule and Runtime sections drawn on their own, with a real
// scope over a fake agent's files: what each field shows, what an edit
// stages, and what a save writes.

let agent = "";
let server: FakeAgentConfig;
let count = 0;

async function loaded(config: string): Promise<AgentScopeModel> {
  agent = `runtime-${String(++count)}`;
  server = fakeAgentConfig(agent, { config });
  mockFetch(server.handler);
  const scope = settingsModel.agent(agent);
  await scope.load();
  return scope;
}

/** The JSON of every config PATCH the page sent, in order. */
const patches = (): unknown[] =>
  server.requests
    .filter((request) => request.method === "PATCH" && request.url.endsWith("/config/patch"))
    .map((request) => JSON.parse(request.body ?? "null") as unknown);

const box = (label: string): HTMLInputElement => screen.getByLabelText(label);
const flag = (label: string): HTMLElement => screen.getByRole("switch", { name: label });

/** Answer every config save with a warning on `path`, as the server does for a value it accepts but doubts. */
function warnOn(path: string, message: string): void {
  mockFetch(async (url, init) => {
    const answer = await server.handler(url, init);
    if (init?.method !== "PATCH") return answer;
    const body = (await answer.json()) as Record<string, unknown>;
    return jsonResponse({
      ...body,
      diagnostics: [{ severity: "warning", message, location: { kind: "path", path } }],
    });
  });
}

async function type(label: string, value: string): Promise<void> {
  await fireEvent.input(box(label), { target: { value } });
  await settle();
}

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

describe("Runtime", () => {
  it("shows the file's numbers with their units, and the default in a box left blank", async () => {
    const scope = await loaded("timeout_secs = 30\n[retry]\nmax_retries = 5\n");
    render(Runtime, { scope, section: "runtime" });

    expect(box("Reply time limit").value).toBe("30");
    expect(screen.getByText("seconds")).toBeTruthy();
    expect(box("Tries after a failure").value).toBe("5");

    const length = box("Reply length");
    expect(length.value).toBe("");
    expect(length.placeholder).toBe("8192");
    expect(length).toHaveAccessibleDescription(/Default: 8,192 tokens\./);
    expect(box("First pause")).toHaveAccessibleDescription(/Default: 500 milliseconds\./);
    expect(box("Tool calls per turn").placeholder).toBe("No limit");
  });

  it("stages an edit as one key and saves only that key", async () => {
    const scope = await loaded("timeout_secs = 30\n");
    render(Runtime, { scope, section: "runtime" });

    await type("Reply time limit", "90");
    expect(scope.dirty).toBe(true);
    expect(patches()).toEqual([]);

    await save(scope);
    expect(patches()).toEqual([{ timeout_secs: 90 }]);
    expect(scope.dirty).toBe(false);
  });

  it("removes a key when its box is cleared, so the agent uses its default again", async () => {
    const scope = await loaded("timeout_secs = 30\n");
    render(Runtime, { scope, section: "runtime" });

    await type("Reply time limit", "");
    await save(scope);

    expect(patches()).toEqual([{ timeout_secs: null }]);
  });

  it("saves a decimal and a count of tool calls as numbers", async () => {
    const scope = await loaded("");
    render(Runtime, { scope, section: "runtime" });

    await type("Each pause lasts", "1.5");
    await type("Tool calls per turn", "40");
    await save(scope);

    expect(patches()).toEqual([
      { retry: { backoff_multiplier: 1.5 }, agent: { max_tool_iterations: 40 } },
    ]);
  });

  it("turns the repeat thresholds off with the guard, and keeps their values", async () => {
    const scope = await loaded("[agent]\nrepeat_call_steer_after = 4\n");
    render(Runtime, { scope, section: "runtime" });

    expect(box("Nudge after")).toBeEnabled();
    expect(box("Nudge after").value).toBe("4");

    await fireEvent.click(flag("Catch repeated tool calls"));
    await settle();
    expect(box("Nudge after")).toBeDisabled();
    expect(box("Stop after")).toBeDisabled();
    expect(box("Nudge after").value).toBe("4");

    await save(scope);
    expect(patches()).toEqual([{ agent: { repeat_call_guard_enabled: false } }]);
  });

  it("lists the idle channels, keeps one the list doesn't know, and saves a new choice", async () => {
    const scope = await loaded('[idle]\nidle_channel = "ws"\n');
    render(Runtime, { scope, section: "runtime" });

    const channel = screen.getByLabelText("Send its updates to");
    expect([...channel.querySelectorAll("option")].map((option) => option.textContent)).toEqual([
      "Keep where it is",
      "This app",
      "Telegram",
      "Discord",
      "Microsoft Teams",
      "ws",
    ]);
    expect((channel as HTMLSelectElement).value).toBe("ws");

    await fireEvent.change(channel, { target: { value: "telegram" } });
    await settle();
    await save(scope);
    expect(patches()).toEqual([{ idle: { idle_channel: "telegram" } }]);
  });

  it("shows a problem the server places on a number under that number", async () => {
    const scope = await loaded("");
    warnOn("agent.max_tool_iterations", "a turn this short can't finish a tool call");
    render(Runtime, { scope, section: "runtime" });

    await type("Tool calls per turn", "1");
    await save(scope);

    expect(box("Tool calls per turn")).toBeInvalid();
    expect(box("Tool calls per turn")).toHaveAccessibleDescription(/can't finish a tool call/);
    expect(box("Reply time limit")).not.toHaveAccessibleDescription(/can't finish/);
  });

  it("needs no running agent: every field is in the agent's config file", async () => {
    const scope = await loaded("");
    render(Runtime, { scope, section: "runtime" });

    expect(screen.queryByText(/to see/)).toBeNull();
    expect(box("Reply time limit")).toBeEnabled();
  });
});

describe("Memory", () => {
  it("keeps the reviewing settings off until Review replies is on", async () => {
    const scope = await loaded("");
    render(Memory, { scope, section: "memory" });

    expect(flag("Review replies")).toHaveAttribute("aria-checked", "false");
    expect(flag("Also review while it works")).toBeDisabled();
    expect(box("Look in every")).toBeDisabled();
    expect(box("Conversation it reads")).toBeDisabled();
    expect(flag("Learn from reviewed replies")).toBeDisabled();

    await fireEvent.click(flag("Review replies"));
    await settle();
    expect(flag("Also review while it works")).toBeEnabled();
    expect(box("Look in every")).toBeEnabled();
    expect(box("Conversation it reads")).toBeEnabled();
    expect(flag("Learn from reviewed replies")).toBeEnabled();

    await fireEvent.click(flag("Also review while it works"));
    await settle();
    expect(box("Look in every")).toBeDisabled();

    await save(scope);
    expect(patches()).toEqual([{ subconscious: { enabled: true, mid_turn: false } }]);
  });

  it("shows the summarizing numbers with tokens and seconds, and writes a change", async () => {
    const scope = await loaded("[memory]\nobserver_threshold_tokens = 30000\n");
    render(Memory, { scope, section: "memory" });

    expect(box("Start summarizing at").value).toBe("30000");
    expect(box("Wait before summarizing")).toHaveAccessibleDescription(/Default: 120 seconds\./);
    expect(box("Condense memories at")).toHaveAccessibleDescription(/Default: 40,000 tokens\./);

    await type("Start summarizing at", "45000");
    await save(scope);
    expect(patches()).toEqual([{ memory: { observer_threshold_tokens: 45000 } }]);
  });

  it("holds the search tuning under More options and writes it under [memory.search]", async () => {
    const scope = await loaded("");
    render(Memory, { scope, section: "memory" });

    const more = screen.getByRole("button", { name: "More options" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    await fireEvent.click(more);
    expect(more).toHaveAttribute("aria-expanded", "true");

    expect(box("Memories lose half their rank after")).toBeDisabled();
    await fireEvent.click(flag("Prefer recent memories"));
    await settle();
    expect(box("Memories lose half their rank after")).toBeEnabled();
    await type("Weight of meaning", "0.8");
    await save(scope);

    expect(patches()).toEqual([
      { memory: { search: { vector_weight: 0.8, temporal_decay: true } } },
    ]);
  });

  it("opens More options when a save finds a problem inside it", async () => {
    const scope = await loaded("");
    warnOn("memory.search.min_score", "scores only run from 0 to 1");
    render(Memory, { scope, section: "memory" });

    const more = screen.getByRole("button", { name: "More options" });
    await fireEvent.click(more);
    await type("Lowest score to show", "7");
    await fireEvent.click(more);
    expect(more).toHaveAttribute("aria-expanded", "false");

    await save(scope);
    await settle();

    expect(more).toHaveAttribute("aria-expanded", "true");
    expect(box("Lowest score to show")).toHaveAccessibleDescription(/scores only run from 0 to 1/);
  });

  it("goes to the Model section to choose the reviewing model", async () => {
    const scope = await loaded("");
    const go = vi.spyOn(router, "switchSettingsSection").mockResolvedValue(true);
    render(Memory, { scope, section: "memory" });

    await fireEvent.click(
      screen.getByRole("button", { name: "Choose the model that reviews replies" }),
    );

    expect(go).toHaveBeenCalledWith("model");
  });
});

describe("Schedule", () => {
  it("turns pulses off and writes [pulse] enabled", async () => {
    const scope = await loaded("[pulse]\nenabled = true\n");
    render(Schedule, { scope, section: "schedule" });

    expect(flag("Run pulses")).toHaveAttribute("aria-checked", "true");
    await fireEvent.click(flag("Run pulses"));
    await settle();
    await save(scope);

    expect(patches()).toEqual([{ pulse: { enabled: false } }]);
  });

  it("shows each kind of session's idle time in minutes, and writes one change", async () => {
    const scope = await loaded("[background]\nidle_timeout_spawned_minutes = 15\n");
    render(Schedule, { scope, section: "schedule" });

    expect(box("Helper sessions").value).toBe("15");
    expect(box("Chat conversations")).toHaveAccessibleDescription(/Default: 30 minutes\./);
    expect(box("Pulses, scheduled actions and webhooks").placeholder).toBe("2");
    expect(box("Shortest session worth remembering")).toHaveAccessibleDescription(
      /Default: 2,000 tokens\./,
    );

    await type("Workbench artifacts", "20");
    await type("How deep helpers can nest", "4");
    await save(scope);
    expect(patches()).toEqual([
      {
        background: { idle_timeout_artifact_minutes: 20, subagent_depth_cap: 4 },
      },
    ]);
  });

  it("opens the agent's Schedule place", async () => {
    const scope = await loaded("");
    const open = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    render(Schedule, { scope, section: "schedule" });

    await fireEvent.click(screen.getByRole("button", { name: `Open ${agent}'s Schedule` }));

    expect(open).toHaveBeenCalledWith({ kind: "schedule", agent });
  });
});
