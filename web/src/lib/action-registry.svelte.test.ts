import { describe, expect, it, vi } from "vitest";
import {
  ActionRegistry,
  commandActions,
  groupRuns,
  matchActions,
  readCommandLine,
  type AppAction,
} from "./action-registry.svelte";

function action(id: string, overrides: Partial<AppAction> = {}): AppAction {
  return { id, group: "Actions", label: id, icon: "info", run: vi.fn(), ...overrides };
}

const ACTIONS: readonly AppAction[] = [
  action("home", { group: "Go to", label: "Home", hint: "Your team" }),
  action("scout-files", { group: "In scout", label: "Files", hint: "scout", searchOnly: true }),
  action("observe", { label: "Summarize older messages now", hint: "atlas", command: "observe" }),
  action("inbox", { label: "Add a note to atlas's inbox", command: "inbox", takesText: true }),
  action("atlas", { group: "Agents", label: "atlas", terms: ["Keeps the team wiki tidy"] }),
];

const labels = (actions: readonly AppAction[]): string[] => actions.map((a) => a.label);

describe("matchActions", () => {
  it("lists everything but the search-only actions when nothing is typed", () => {
    expect(labels(matchActions(ACTIONS, "  "))).toEqual([
      "Home",
      "Summarize older messages now",
      "Add a note to atlas's inbox",
      "atlas",
    ]);
  });

  it("finds a chat action by its old slash command", () => {
    expect(labels(matchActions(ACTIONS, "/observe"))).toEqual(["Summarize older messages now"]);
    expect(labels(matchActions(ACTIONS, "obs"))).toEqual(["Summarize older messages now"]);
  });

  it("needs every word, in any order, across label, hint, heading and terms", () => {
    expect(labels(matchActions(ACTIONS, "older summarize"))).toEqual([
      "Summarize older messages now",
    ]);
    expect(labels(matchActions(ACTIONS, "scout files"))).toEqual(["Files"]);
    expect(labels(matchActions(ACTIONS, "wiki"))).toEqual(["atlas"]);
    expect(matchActions(ACTIONS, "older wiki")).toEqual([]);
  });
});

describe("readCommandLine", () => {
  const commands = commandActions(ACTIONS);

  it("names the action and passes the rest of the line as its text", () => {
    const line = readCommandLine(commands, "/inbox  check the  wiki ");
    expect(line?.action?.id).toBe("inbox");
    expect(line?.text).toBe("check the  wiki");
  });

  it("matches the command whatever its case, and reads plain messages as none", () => {
    expect(readCommandLine(commands, "/OBSERVE")?.action?.id).toBe("observe");
    expect(readCommandLine(commands, "hello /observe")).toBeNull();
  });

  it("keeps an unknown command's name, with no action", () => {
    expect(readCommandLine(commands, "/verbos on")).toEqual({
      name: "verbos",
      action: null,
      text: "on",
    });
  });
});

describe("groupRuns", () => {
  it("gathers neighbors under one heading and keeps each action's index", () => {
    const runs = groupRuns(matchActions(ACTIONS, ""));
    expect(runs.map((run) => run.heading)).toEqual(["Go to", "Actions", "Agents"]);
    expect(runs[1]?.actions.map((entry) => entry.index)).toEqual([1, 2]);
  });
});

describe("ActionRegistry", () => {
  it("lists its sources' actions in the order they registered, and replaces one by key", () => {
    const registry = new ActionRegistry();
    registry.register("a", () => [action("a1")]);
    const removeB = registry.register("b", () => [action("b1")]);
    registry.register("a", () => [action("a2")]);
    expect(registry.all.map((a) => a.id)).toEqual(["a2", "b1"]);
    removeB();
    expect(registry.all.map((a) => a.id)).toEqual(["a2"]);
  });

  it("tells its listeners before running an action, and never runs a disabled one", async () => {
    const registry = new ActionRegistry();
    const order: string[] = [];
    registry.onRun(() => order.push("listener"));
    const run = vi.fn(() => order.push("run"));

    expect(await registry.run(action("on", { run }), "text")).toBe(true);
    expect(order).toEqual(["listener", "run"]);
    expect(run).toHaveBeenCalledWith("text");

    expect(await registry.run(action("off", { run, disabled: "Start atlas first" }))).toBe(false);
    expect(run).toHaveBeenCalledTimes(1);
    expect(order).toEqual(["listener", "run"]);
  });
});
