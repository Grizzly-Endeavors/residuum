import { describe, expect, it } from "vitest";
import { formatLocation, MAIN_CHAT, parseLocation, type AppLocation } from "./routes";

const IN_SESSION = { runId: "run-1790000000000-0a1b2c3d", workspace: true };

describe("parseLocation", () => {
  it("reads the root as the main chat", () => {
    expect(parseLocation("/", "", IN_SESSION)).toEqual({
      location: { chat: MAIN_CHAT, settings: null, workbench: null, scheduled: false },
      corrected: false,
    });
  });

  it("reads a session path and the workspace flag", () => {
    expect(parseLocation("/sessions/run-1-ab", "?workspace", MAIN_CHAT)).toEqual({
      location: {
        chat: { runId: "run-1-ab", workspace: true },
        settings: null,
        workbench: null,
        scheduled: false,
      },
      corrected: false,
    });
  });

  it("decodes an encoded run id", () => {
    const { location } = parseLocation("/sessions/run%20x", "", MAIN_CHAT);
    expect(location.chat.runId).toBe("run x");
  });

  it("keeps the chat side as it was when opening settings", () => {
    expect(parseLocation("/settings/memory", "", IN_SESSION)).toEqual({
      location: { chat: IN_SESSION, settings: "memory", workbench: null, scheduled: false },
      corrected: false,
    });
  });

  it("sends bare /settings to the first section", () => {
    expect(parseLocation("/settings", "", MAIN_CHAT)).toEqual({
      location: { chat: MAIN_CHAT, settings: "runtime", workbench: null, scheduled: false },
      corrected: true,
    });
  });

  it("corrects an unknown settings section to the first section", () => {
    expect(parseLocation("/settings/nope", "", MAIN_CHAT)).toEqual({
      location: { chat: MAIN_CHAT, settings: "runtime", workbench: null, scheduled: false },
      corrected: true,
    });
  });

  it.each(["/nope", "/sessions", "/sessions/a/b", "/sessions/%E0%A4%A", "/settings/runtime/x"])(
    "corrects %s to the main chat",
    (path) => {
      expect(parseLocation(path, "", IN_SESSION)).toEqual({
        location: { chat: MAIN_CHAT, settings: null, workbench: null, scheduled: false },
        corrected: true,
      });
    },
  );

  it("reads the workbench list and keeps the chat side", () => {
    expect(parseLocation("/workbench", "", IN_SESSION)).toEqual({
      location: {
        chat: IN_SESSION,
        settings: null,
        workbench: { artifact: null, full: false },
        scheduled: false,
      },
      corrected: false,
    });
  });

  it("reads a workbench artifact", () => {
    expect(parseLocation("/workbench/pricing-explorer", "", MAIN_CHAT)).toEqual({
      location: {
        chat: MAIN_CHAT,
        settings: null,
        workbench: { artifact: "pricing-explorer", full: false },
        scheduled: false,
      },
      corrected: false,
    });
  });

  it.each(["/workbench/Bad_Name", "/workbench/a--b", "/workbench/%E0%A4%A", "/workbench/x.html"])(
    "corrects %s to the workbench list",
    (path) => {
      expect(parseLocation(path, "", MAIN_CHAT)).toEqual({
        location: {
          chat: MAIN_CHAT,
          settings: null,
          workbench: { artifact: null, full: false },
          scheduled: false,
        },
        corrected: true,
      });
    },
  );

  it("reads an artifact in full view", () => {
    expect(parseLocation("/workbench/chart", "?full", MAIN_CHAT)).toEqual({
      location: {
        chat: MAIN_CHAT,
        settings: null,
        workbench: { artifact: "chart", full: true },
        scheduled: false,
      },
      corrected: false,
    });
  });

  it("drops full view from the artifact list", () => {
    expect(parseLocation("/workbench", "?full", MAIN_CHAT)).toEqual({
      location: {
        chat: MAIN_CHAT,
        settings: null,
        workbench: { artifact: null, full: false },
        scheduled: false,
      },
      corrected: true,
    });
  });

  it("corrects a nested workbench path to the main chat", () => {
    expect(parseLocation("/workbench/a/b", "", MAIN_CHAT).location.workbench).toBeNull();
  });

  it("tolerates a trailing slash", () => {
    expect(parseLocation("/sessions/run-1/", "", MAIN_CHAT).corrected).toBe(false);
  });

  it("reads the scheduled view and keeps the chat side", () => {
    expect(parseLocation("/scheduled", "", IN_SESSION)).toEqual({
      location: { chat: IN_SESSION, settings: null, workbench: null, scheduled: true },
      corrected: false,
    });
  });

  it("corrects a nested scheduled path to the main chat", () => {
    expect(parseLocation("/scheduled/x", "", MAIN_CHAT).location.scheduled).toBe(false);
  });
});

describe("formatLocation", () => {
  it.each<[AppLocation, string]>([
    [{ chat: MAIN_CHAT, settings: null, workbench: null, scheduled: false }, "/"],
    [
      { chat: { runId: null, workspace: true }, settings: null, workbench: null, scheduled: false },
      "/?workspace",
    ],
    [
      {
        chat: { runId: "run-1", workspace: false },
        settings: null,
        workbench: null,
        scheduled: false,
      },
      "/sessions/run-1",
    ],
    [
      {
        chat: { runId: "run-1", workspace: true },
        settings: null,
        workbench: null,
        scheduled: false,
      },
      "/sessions/run-1?workspace",
    ],
    [
      { chat: IN_SESSION, settings: "agent-keys", workbench: null, scheduled: false },
      "/settings/agent-keys",
    ],
    [
      {
        chat: IN_SESSION,
        settings: null,
        workbench: { artifact: null, full: false },
        scheduled: false,
      },
      "/workbench",
    ],
    [
      {
        chat: MAIN_CHAT,
        settings: null,
        workbench: { artifact: "chart", full: false },
        scheduled: false,
      },
      "/workbench/chart",
    ],
    [
      {
        chat: MAIN_CHAT,
        settings: null,
        workbench: { artifact: "chart", full: true },
        scheduled: false,
      },
      "/workbench/chart?full",
    ],
    [{ chat: MAIN_CHAT, settings: null, workbench: null, scheduled: true }, "/scheduled"],
  ])("formats %j as %s", (location, url) => {
    expect(formatLocation(location)).toBe(url);
  });

  it("round-trips through parseLocation", () => {
    const location: AppLocation = {
      chat: { runId: "run/odd id", workspace: true },
      settings: null,
      workbench: null,
      scheduled: false,
    };
    const url = new URL(formatLocation(location), "http://localhost");
    expect(parseLocation(url.pathname, url.search, MAIN_CHAT)).toEqual({
      location,
      corrected: false,
    });
  });
});
