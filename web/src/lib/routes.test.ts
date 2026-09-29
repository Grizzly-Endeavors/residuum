import { describe, expect, it } from "vitest";
import {
  formatLocation,
  MAIN_CHAT,
  parseLocation,
  type AppLocation,
  type ChatLocation,
  type LocationContext,
} from "./routes";

const IN_SESSION: ChatLocation = { runId: "run-1790000000000-0a1b2c3d", workspace: true };

/** Reading a URL while on `scout`, with `scout` also the last-used agent. */
function ctx(chat: ChatLocation = MAIN_CHAT, agent: string | null = "scout"): LocationContext {
  return { agent, chat, fallbackAgent: "scout" };
}

function at(overrides: Partial<AppLocation> = {}): AppLocation {
  return {
    agent: "scout",
    chat: MAIN_CHAT,
    settings: null,
    workbench: null,
    scheduled: false,
    team: null,
    ...overrides,
  };
}

describe("parseLocation: agent pages", () => {
  it("reads an agent path as that agent's main chat", () => {
    expect(parseLocation("/agent/scout", "", ctx())).toEqual({
      location: at(),
      corrected: false,
    });
  });

  it("takes the agent from the URL, not from where the user was", () => {
    const { location } = parseLocation("/agent/atlas", "", ctx(IN_SESSION));
    expect(location.agent).toBe("atlas");
    expect(location.chat).toEqual(MAIN_CHAT);
  });

  it("reads a session path and the workspace flag", () => {
    expect(parseLocation("/agent/scout/sessions/run-1-ab", "?workspace", ctx())).toEqual({
      location: at({ chat: { runId: "run-1-ab", workspace: true } }),
      corrected: false,
    });
  });

  it("decodes an encoded run id", () => {
    const { location } = parseLocation("/agent/scout/sessions/run%20x", "", ctx());
    expect(location.chat.runId).toBe("run x");
  });

  it("reads the workspace page", () => {
    expect(parseLocation("/agent/scout/workspace", "", ctx())).toEqual({
      location: at({ chat: { runId: null, workspace: true } }),
      corrected: false,
    });
  });

  it("reads the scheduled view and keeps the chat side of the same agent", () => {
    expect(parseLocation("/agent/scout/scheduled", "", ctx(IN_SESSION))).toEqual({
      location: at({ chat: IN_SESSION, scheduled: true }),
      corrected: false,
    });
  });

  it("drops another agent's open session when opening a page of a different agent", () => {
    const { location } = parseLocation("/agent/atlas/scheduled", "", ctx(IN_SESSION));
    expect(location.chat).toEqual(MAIN_CHAT);
  });

  it("reads agent settings and keeps the chat side", () => {
    expect(parseLocation("/agent/scout/settings/memory", "", ctx(IN_SESSION))).toEqual({
      location: at({ chat: IN_SESSION, settings: { scope: "agent", section: "memory" } }),
      corrected: false,
    });
  });

  it.each(["/agent/scout/settings", "/agent/scout/settings/nope"])(
    "sends %s to the first settings section",
    (path) => {
      expect(parseLocation(path, "", ctx())).toEqual({
        location: at({ settings: { scope: "agent", section: "runtime" } }),
        corrected: true,
      });
    },
  );

  it.each([
    "/agent/scout/nope",
    "/agent/scout/sessions",
    "/agent/scout/sessions/a/b",
    "/agent/scout/sessions/%E0%A4%A",
    "/agent/scout/settings/runtime/x",
    "/agent/scout/scheduled/x",
  ])("corrects %s to the agent's main chat", (path) => {
    expect(parseLocation(path, "", ctx(IN_SESSION))).toEqual({
      location: at(),
      corrected: true,
    });
  });

  it("corrects a malformed agent name to the agent in context", () => {
    expect(parseLocation("/agent/Bad_Name", "", ctx())).toEqual({
      location: at(),
      corrected: true,
    });
  });

  it("corrects a bare /agent to the agent in context", () => {
    expect(parseLocation("/agent", "", ctx())).toEqual({ location: at(), corrected: true });
  });

  it("tolerates a trailing slash", () => {
    expect(parseLocation("/agent/scout/sessions/run-1/", "", ctx()).corrected).toBe(false);
  });
});

describe("parseLocation: team pages", () => {
  it("reads /team as the overview and keeps the agent and chat side", () => {
    expect(parseLocation("/team", "", ctx(IN_SESSION))).toEqual({
      location: at({ chat: IN_SESSION, team: "overview" }),
      corrected: false,
    });
  });

  it("reads team files", () => {
    expect(parseLocation("/team/files", "", ctx()).location.team).toBe("files");
  });

  it("reads the workbench list", () => {
    expect(parseLocation("/team/workbench", "", ctx(IN_SESSION))).toEqual({
      location: at({ chat: IN_SESSION, workbench: { artifact: null, full: false } }),
      corrected: false,
    });
  });

  it("reads a workbench artifact", () => {
    expect(parseLocation("/team/workbench/pricing-explorer", "", ctx()).location.workbench).toEqual(
      { artifact: "pricing-explorer", full: false },
    );
  });

  it.each([
    "/team/workbench/Bad_Name",
    "/team/workbench/a--b",
    "/team/workbench/%E0%A4%A",
    "/team/workbench/x.html",
  ])("corrects %s to the workbench list", (path) => {
    expect(parseLocation(path, "", ctx())).toEqual({
      location: at({ workbench: { artifact: null, full: false } }),
      corrected: true,
    });
  });

  it("reads an artifact in full view", () => {
    expect(parseLocation("/team/workbench/chart", "?full", ctx()).location.workbench).toEqual({
      artifact: "chart",
      full: true,
    });
  });

  it("drops full view from the artifact list", () => {
    expect(parseLocation("/team/workbench", "?full", ctx())).toEqual({
      location: at({ workbench: { artifact: null, full: false } }),
      corrected: true,
    });
  });

  it("reads hub settings", () => {
    expect(parseLocation("/team/settings/agent-keys", "", ctx())).toEqual({
      location: at({ settings: { scope: "hub", section: "agent-keys" } }),
      corrected: false,
    });
  });

  it("sends bare /team/settings to the first section", () => {
    expect(parseLocation("/team/settings", "", ctx())).toEqual({
      location: at({ settings: { scope: "hub", section: "runtime" } }),
      corrected: true,
    });
  });

  it("corrects an unknown team page to the overview", () => {
    expect(parseLocation("/team/nope", "", ctx())).toEqual({
      location: at({ team: "overview" }),
      corrected: true,
    });
  });

  it("uses the last-used agent when none is in context", () => {
    const { location } = parseLocation("/team/files", "", { ...ctx(), agent: null });
    expect(location.agent).toBe("scout");
  });

  it("has no agent when none is in context and none was used", () => {
    const { location } = parseLocation("/team", "", {
      agent: null,
      chat: MAIN_CHAT,
      fallbackAgent: null,
    });
    expect(location.agent).toBeNull();
  });
});

describe("parseLocation: root and legacy paths", () => {
  it("sends / to the last-used agent", () => {
    expect(parseLocation("/", "", { ...ctx(), agent: null })).toEqual({
      location: at(),
      corrected: true,
    });
  });

  it("leaves / alone when there is no agent to go to", () => {
    expect(parseLocation("/", "", { agent: null, chat: MAIN_CHAT, fallbackAgent: null })).toEqual({
      location: at({ agent: null }),
      corrected: false,
    });
  });

  it("sends / to the agent in context ahead of the last-used one", () => {
    const { location } = parseLocation("/", "", {
      agent: "atlas",
      chat: MAIN_CHAT,
      fallbackAgent: "scout",
    });
    expect(location.agent).toBe("atlas");
  });

  it.each([
    ["/settings/memory", at({ settings: { scope: "agent", section: "memory" } })],
    ["/workbench/chart", at({ workbench: { artifact: "chart", full: false } })],
    ["/scheduled", at({ scheduled: true })],
    ["/sessions/run-1", at({ chat: { runId: "run-1", workspace: false } })],
  ])("moves the older path %s under the agent in context", (path, location) => {
    expect(parseLocation(path, "", ctx())).toEqual({ location, corrected: true });
  });

  it.each(["/nope", "/sessions", "/sessions/a/b"])("corrects %s to the main chat", (path) => {
    expect(parseLocation(path, "", ctx(IN_SESSION))).toEqual({ location: at(), corrected: true });
  });
});

describe("formatLocation", () => {
  it.each<[AppLocation, string]>([
    [at(), "/agent/scout"],
    [at({ chat: { runId: null, workspace: true } }), "/agent/scout/workspace"],
    [at({ chat: { runId: "run-1", workspace: false } }), "/agent/scout/sessions/run-1"],
    [at({ chat: { runId: "run-1", workspace: true } }), "/agent/scout/sessions/run-1?workspace"],
    [at({ scheduled: true }), "/agent/scout/scheduled"],
    [at({ settings: { scope: "agent", section: "memory" } }), "/agent/scout/settings/memory"],
    [at({ settings: { scope: "hub", section: "agent-keys" } }), "/team/settings/agent-keys"],
    [at({ team: "overview" }), "/team"],
    [at({ team: "files" }), "/team/files"],
    [at({ workbench: { artifact: null, full: false } }), "/team/workbench"],
    [at({ workbench: { artifact: "chart", full: false } }), "/team/workbench/chart"],
    [at({ workbench: { artifact: "chart", full: true } }), "/team/workbench/chart?full"],
    [at({ agent: null }), "/"],
    [at({ agent: "a b" }), "/agent/a%20b"],
  ])("formats %j as %s", (location, url) => {
    expect(formatLocation(location)).toBe(url);
  });

  it("keeps hub settings and the team pages reachable with no agent", () => {
    expect(formatLocation(at({ agent: null, team: "overview" }))).toBe("/team");
    expect(
      formatLocation(at({ agent: null, settings: { scope: "hub", section: "runtime" } })),
    ).toBe("/team/settings/runtime");
  });

  it.each<AppLocation>([
    at({ chat: { runId: "run/odd id", workspace: true } }),
    at({ chat: { runId: null, workspace: true } }),
    at({ scheduled: true }),
    at({ settings: { scope: "agent", section: "history" } }),
    at({ settings: { scope: "hub", section: "a2a" } }),
    at({ team: "files" }),
    at({ workbench: { artifact: "chart", full: true } }),
  ])("round-trips %j through parseLocation", (location) => {
    const url = new URL(formatLocation(location), "http://localhost");
    expect(parseLocation(url.pathname, url.search, ctx(location.chat))).toEqual({
      location,
      corrected: false,
    });
  });
});
