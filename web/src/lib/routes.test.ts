import { describe, expect, it } from "vitest";
import {
  correctForAgents,
  correctForArtifacts,
  formatLocation,
  HOME,
  isAgentName,
  isArtifactName,
  locationAt,
  locationsEqual,
  panelAllowed,
  parseUrl,
  viewedAgentOf,
  type AppLocation,
  type Panel,
  type ParsedUrl,
  type Place,
} from "./routes";

/** Read a URL with `scout` as the last-used agent, and format what it reads: the address the app settles on. */
function read(url: string, lastUsed: string | null = "scout"): ParsedUrl & { url: string } {
  const [path = "", query = ""] = url.split("?");
  const parsed = parseUrl(path, query === "" ? "" : `?${query}`, { lastUsed });
  return { ...parsed, url: formatLocation(parsed.location) };
}

function where(url: string, lastUsed: string | null = "scout"): string {
  return read(url, lastUsed).url;
}

const session = (agent: string, runId: string): Panel => ({ kind: "session", agent, runId });

describe("places", () => {
  it.each<[string, Place]>([
    ["/home", { kind: "home" }],
    ["/inbox", { kind: "inbox", agent: null, tab: "active", item: null }],
    ["/agent/scout", { kind: "chat", agent: "scout" }],
    ["/agent/scout/activity", { kind: "activity", agent: "scout" }],
    ["/agent/scout/schedule", { kind: "schedule", agent: "scout" }],
    ["/agent/scout/files", { kind: "files", agent: "scout" }],
    ["/team/workbench", { kind: "workbench", artifact: null }],
    ["/team/workbench/tip-splitter", { kind: "workbench", artifact: "tip-splitter" }],
    ["/team/files", { kind: "shared-files" }],
  ])("reads %s", (url, place) => {
    const parsed = read(url);
    expect(parsed.location).toEqual({ place, panel: null, settings: null });
    expect(parsed.url).toBe(url);
    expect(parsed.notices).toEqual([]);
    expect(parsed.needsLastUsed).toBe(false);
  });

  it("sends / to /home", () => {
    expect(read("/").location.place).toEqual({ kind: "home" });
    expect(where("/")).toBe("/home");
  });

  it("names the viewed agent only on agent places", () => {
    expect(viewedAgentOf({ kind: "chat", agent: "scout" })).toBe("scout");
    expect(viewedAgentOf({ kind: "files", agent: "atlas" })).toBe("atlas");
    expect(viewedAgentOf(HOME)).toBeNull();
    expect(viewedAgentOf({ kind: "workbench", artifact: null })).toBeNull();
    expect(viewedAgentOf({ kind: "shared-files" })).toBeNull();
    expect(viewedAgentOf({ kind: "inbox", agent: "scout", tab: "active", item: null })).toBeNull();
  });

  it("decodes encoded segments", () => {
    expect(read("/team/workbench/tip%2Dsplitter").location.place).toEqual({
      kind: "workbench",
      artifact: "tip-splitter",
    });
  });
});

describe("unknown routes", () => {
  it.each([
    ["/nowhere", "/home"],
    ["/home/extra", "/home"],
    ["/inbox/extra", "/home"],
    ["/agent", "/home"],
    ["/agent/Bad_Name", "/home"],
    ["/agent/scout/nowhere", "/home"],
    ["/agent/scout/files/extra", "/home"],
    ["/team/nowhere", "/home"],
    ["/team/files/extra", "/home"],
    ["/%E0%A4%A", "/home"],
  ])("corrects %s to %s", (from, to) => {
    expect(where(from)).toBe(to);
  });

  it("corrects a malformed artifact name to the list", () => {
    expect(where("/team/workbench/Bad_Name")).toBe("/team/workbench");
    expect(where("/team/workbench/a/b")).toBe("/home");
  });

  it("accepts the artifact-name rule's limits", () => {
    expect(isArtifactName("a")).toBe(true);
    expect(isArtifactName("a-b-c")).toBe(true);
    expect(isArtifactName("-a")).toBe(false);
    expect(isArtifactName("a--b")).toBe(false);
    expect(isArtifactName("a".repeat(65))).toBe(false);
    expect(isAgentName("scout")).toBe(true);
    expect(isAgentName("Scout")).toBe(false);
  });

  it("drops query parameters it doesn't know", () => {
    expect(where("/home?utm=1")).toBe("/home");
  });

  it("settles a trailing slash", () => {
    expect(where("/agent/scout/")).toBe("/agent/scout");
  });
});

describe("the inbox parameters", () => {
  it("reads the agent filter, the archive tab and an open item", () => {
    expect(read("/inbox?agent=atlas&tab=archived&item=atlas:note-1").location.place).toEqual({
      kind: "inbox",
      agent: "atlas",
      tab: "archived",
      item: { agent: "atlas", id: "note-1" },
    });
  });

  it("keeps an item's id whole when it has colons or needs encoding", () => {
    const { location, url } = read("/inbox?item=atlas:2026-09-30T10:00:00%20x");
    expect(location.place).toEqual({
      kind: "inbox",
      agent: null,
      tab: "active",
      item: { agent: "atlas", id: "2026-09-30T10:00:00 x" },
    });
    expect(where(url)).toBe(url);
  });

  it("removes a parameter that can't be read", () => {
    expect(where("/inbox?agent=Bad_Name")).toBe("/inbox");
    expect(where("/inbox?tab=other")).toBe("/inbox");
    expect(where("/inbox?item=nocolon")).toBe("/inbox");
    expect(where("/inbox?item=:id")).toBe("/inbox");
    expect(where("/inbox?item=atlas:")).toBe("/inbox");
    expect(where("/inbox?item=Bad_Name:id")).toBe("/inbox");
  });

  it("writes the parameters in a fixed order", () => {
    const place: Place = {
      kind: "inbox",
      agent: "atlas",
      tab: "archived",
      item: { agent: "scout", id: "x" },
    };
    expect(formatLocation(locationAt(place))).toBe("/inbox?agent=atlas&tab=archived&item=scout:x");
  });
});

describe("the panel parameter", () => {
  it.each([
    ["/agent/scout?panel=session:scout:run-1", session("scout", "run-1")],
    ["/agent/scout/activity?panel=session:scout:run-1", session("scout", "run-1")],
    ["/agent/scout/schedule?panel=session:scout:run-1", session("scout", "run-1")],
    ["/agent/scout/files?panel=session:scout:run-1", session("scout", "run-1")],
    ["/team/workbench?panel=session:atlas:run-2", session("atlas", "run-2")],
    ["/team/workbench/tip-splitter?panel=session:atlas:run-2", session("atlas", "run-2")],
    ["/agent/scout?panel=file:SOUL.md", { kind: "file", path: "SOUL.md" }],
    ["/agent/scout/files?panel=file:team/USER.md", { kind: "file", path: "team/USER.md" }],
    ["/team/files?panel=file:notes/plan.md", { kind: "file", path: "notes/plan.md" }],
    ["/agent/scout?panel=size", { kind: "size" }],
    ["/agent/scout/files?panel=size", { kind: "size" }],
    ["/agent/scout/activity?panel=new-session:scout", { kind: "new-session", agent: "scout" }],
    ["/agent/scout?panel=new-session:scout", { kind: "new-session", agent: "scout" }],
  ] as [string, Panel][])("keeps %s", (url, panel) => {
    const parsed = read(url);
    expect(parsed.location.panel).toEqual(panel);
    expect(parsed.url).toBe(url);
  });

  it("removes a session or new-session panel for another agent on an agent place", () => {
    expect(where("/agent/scout?panel=session:atlas:run-1")).toBe("/agent/scout");
    expect(where("/agent/scout/activity?panel=new-session:atlas")).toBe("/agent/scout/activity");
    expect(where("/agent/scout/files?panel=session:atlas:run-1")).toBe("/agent/scout/files");
  });

  it("removes the panel on places that can't show it", () => {
    for (const url of [
      "/home?panel=size",
      "/home?panel=file:a.md",
      "/home?panel=session:scout:run-1",
      "/inbox?panel=size",
      "/team/workbench?panel=size",
      "/team/workbench?panel=file:a.md",
      "/team/files?panel=size",
      "/team/files?panel=session:scout:run-1",
      "/team/workbench?panel=new-session:scout",
      "/home?panel=new-session:scout",
      "/agent/scout?panel=file:",
    ]) {
      expect(read(url).location.panel).toBeNull();
    }
    expect(where("/home?panel=size")).toBe("/home");
    expect(where("/team/files?panel=size")).toBe("/team/files");
  });

  it("removes a panel with an unknown kind or a malformed value", () => {
    for (const value of [
      "other:x",
      "nocolon",
      "",
      "session",
      "session:scout",
      "session:scout:",
      "session::run-1",
      "session:Bad_Name:run-1",
      "size:extra",
      "file",
      "new-session",
      "new-session:",
      "new-session:Bad_Name",
    ]) {
      expect(where(`/agent/scout?panel=${value}`)).toBe("/agent/scout");
    }
  });

  it("decodes an encoded value and writes the separators readably", () => {
    const { location, url } = read("/agent/scout/files?panel=file%3Anotes%2Fmy%20plan.md");
    expect(location.panel).toEqual({ kind: "file", path: "notes/my plan.md" });
    expect(url).toBe("/agent/scout/files?panel=file:notes/my%20plan.md");
    expect(where(url)).toBe(url);
  });

  it("keeps a run id that has colons", () => {
    const { location, url } = read("/agent/scout?panel=session:scout:run:1");
    expect(location.panel).toEqual(session("scout", "run:1"));
    expect(where(url)).toBe(url);
  });

  it("says where a panel is valid", () => {
    const chat: Place = { kind: "chat", agent: "scout" };
    expect(panelAllowed(chat, session("scout", "r"))).toBe(true);
    expect(panelAllowed(chat, session("atlas", "r"))).toBe(false);
    expect(panelAllowed({ kind: "workbench", artifact: null }, session("atlas", "r"))).toBe(true);
    expect(panelAllowed(chat, { kind: "new-session", agent: "scout" })).toBe(true);
    expect(panelAllowed(chat, { kind: "new-session", agent: "atlas" })).toBe(false);
    expect(
      panelAllowed({ kind: "workbench", artifact: null }, { kind: "new-session", agent: "atlas" }),
    ).toBe(false);
    expect(panelAllowed(HOME, { kind: "size" })).toBe(false);
    expect(panelAllowed({ kind: "shared-files" }, { kind: "file", path: "a" })).toBe(true);
    expect(panelAllowed({ kind: "shared-files" }, { kind: "size" })).toBe(false);
  });
});

describe("the settings parameter", () => {
  it("reads a scope with a section", () => {
    expect(read("/home?settings=scout/memory").location.settings).toEqual({
      scope: "scout",
      section: "memory",
    });
    expect(read("/home?settings=_all/cloud").location.settings).toEqual({
      scope: "_all",
      section: "cloud",
    });
    expect(where("/agent/scout?settings=scout/memory")).toBe("/agent/scout?settings=scout/memory");
  });

  it("reads a scope alone, leaving the section to the frame's width", () => {
    expect(read("/agent/scout?settings=scout").location.settings).toEqual({
      scope: "scout",
      section: null,
    });
    expect(where("/home?settings=_all")).toBe("/home?settings=_all");
  });

  it("opens over any place, including one of another agent", () => {
    expect(where("/agent/scout/files?settings=atlas/model")).toBe(
      "/agent/scout/files?settings=atlas/model",
    );
    expect(where("/inbox?settings=_all/notifications")).toBe("/inbox?settings=_all/notifications");
    expect(where("/team/files?settings=_all/general")).toBe("/team/files?settings=_all/general");
  });

  it("puts the panel before the modal", () => {
    expect(where("/agent/scout?settings=scout/model&panel=size")).toBe(
      "/agent/scout?panel=size&settings=scout/model",
    );
  });

  it("becomes the scope's default for an unknown section", () => {
    expect(where("/home?settings=scout/nope")).toBe("/home?settings=scout/model");
    expect(where("/home?settings=_all/nope")).toBe("/home?settings=_all/general");
    expect(where("/home?settings=scout/")).toBe("/home?settings=scout/model");
  });

  it("takes a section from the wrong scope as unknown", () => {
    expect(where("/home?settings=_all/model")).toBe("/home?settings=_all/general");
    expect(where("/home?settings=scout/cloud")).toBe("/home?settings=scout/model");
  });

  it("goes to All agents with a notice when the scope can't be an agent", () => {
    const parsed = read("/home?settings=Bad_Name/model");
    expect(parsed.url).toBe("/home?settings=_all/general");
    expect(parsed.notices).toEqual([`There's no agent named "Bad_Name".`]);
  });

  it("removes an empty parameter", () => {
    expect(where("/home?settings=")).toBe("/home");
    expect(where("/home?settings=/model")).toBe("/home");
  });
});

describe("redirects from old URLs", () => {
  it.each([
    ["/team", "/home"],
    ["/agent/scout/sessions/run-1", "/agent/scout?panel=session:scout:run-1"],
    ["/agent/scout/sessions/run-1?workspace", "/agent/scout?panel=session:scout:run-1"],
    ["/agent/atlas/sessions/run-1?workspace", "/agent/atlas?panel=session:atlas:run-1"],
    ["/agent/scout/workspace", "/agent/scout/files"],
    ["/agent/scout?workspace", "/agent/scout/files"],
    ["/agent/scout/scheduled", "/agent/scout/schedule"],
    ["/workbench", "/team/workbench"],
    ["/workbench/tip-splitter", "/team/workbench/tip-splitter"],
    ["/team/workbench/tip-splitter?full", "/team/workbench/tip-splitter"],
    ["/workbench/tip-splitter?full", "/team/workbench/tip-splitter"],
  ])("sends %s to %s", (from, to) => {
    expect(where(from)).toBe(to);
  });

  it("decodes an old run id", () => {
    expect(where("/agent/scout/sessions/run%20x")).toBe("/agent/scout?panel=session:scout:run%20x");
  });

  it("sends a macOS notification's Open link to the last-used agent's Files", () => {
    expect(where("/notification/abc123", "atlas")).toBe("/agent/atlas/files");
    expect(where("/notification/abc123?x=1", "atlas")).toBe("/agent/atlas/files");
  });

  it.each([
    ["/scheduled", "/agent/atlas/schedule"],
    ["/sessions/run-1", "/agent/atlas?panel=session:atlas:run-1"],
    ["/settings", "/agent/atlas?settings=atlas"],
    ["/settings/memory", "/agent/atlas?settings=atlas/memory"],
    ["/settings/agent-keys", "/agent/atlas?settings=_all/keys"],
    ["/settings/integrations", "/agent/atlas?settings=atlas/connections"],
    ["/settings/nope", "/agent/atlas?settings=atlas/model"],
  ])("resolves %s under the last-used agent: %s", (from, to) => {
    expect(where(from, "atlas")).toBe(to);
  });

  it("waits for the last-used agent when it isn't known yet", () => {
    for (const url of [
      "/scheduled",
      "/sessions/run-1",
      "/settings",
      "/settings/memory",
      "/notification/abc",
      "/team/settings/runtime",
    ]) {
      const parsed = read(url, null);
      expect(parsed.needsLastUsed).toBe(true);
      expect(parsed.location).toEqual(locationAt(HOME));
    }
  });

  it("doesn't wait for URLs that need no agent", () => {
    for (const url of [
      "/",
      "/team",
      "/workbench",
      "/team/settings/general",
      "/agent/scout/settings",
    ]) {
      expect(read(url, null).needsLastUsed).toBe(false);
    }
  });

  it.each([
    ["/agent/scout/settings", "/agent/scout?settings=scout"],
    ["/agent/scout/settings/runtime", "/agent/scout?settings=scout/runtime"],
    ["/agent/scout/settings/providers", "/agent/scout?settings=scout/model"],
    ["/agent/scout/settings/channels", "/agent/scout?settings=scout/connections"],
    ["/agent/scout/settings/integrations", "/agent/scout?settings=scout/connections"],
    ["/agent/scout/settings/webhooks", "/agent/scout?settings=scout/connections"],
    ["/agent/scout/settings/pulses", "/agent/scout?settings=scout/schedule"],
    ["/agent/scout/settings/memory", "/agent/scout?settings=scout/memory"],
    ["/agent/scout/settings/skills", "/agent/scout?settings=scout/tools"],
    ["/agent/scout/settings/mcp", "/agent/scout?settings=scout/servers"],
    ["/agent/scout/settings/a2a", "/agent/scout?settings=scout/a2a"],
    ["/agent/scout/settings/history", "/agent/scout?settings=scout/history"],
  ])("sends old agent settings %s to %s", (from, to) => {
    expect(where(from)).toBe(to);
  });

  it.each([
    ["/agent/scout/settings/general", "/agent/scout?settings=_all/general"],
    ["/agent/scout/settings/cloud", "/agent/scout?settings=_all/cloud"],
    ["/agent/scout/settings/sessions", "/agent/scout?settings=_all/limits"],
    ["/agent/scout/settings/tracing", "/agent/scout?settings=_all/diagnostics"],
    ["/agent/scout/settings/update", "/agent/scout?settings=_all/updates"],
    ["/agent/scout/settings/secrets", "/agent/scout?settings=_all/keys"],
    ["/agent/scout/settings/agent-keys", "/agent/scout?settings=_all/keys"],
  ])("moves %s to the install-wide scope: %s", (from, to) => {
    expect(where(from)).toBe(to);
  });

  it("sends an unknown old agent section to the agent's default", () => {
    expect(where("/agent/scout/settings/nope")).toBe("/agent/scout?settings=scout/model");
  });

  it.each([
    ["/team/settings", "/home?settings=_all"],
    ["/team/settings/general", "/home?settings=_all/general"],
    ["/team/settings/cloud", "/home?settings=_all/cloud"],
    ["/team/settings/a2a", "/home?settings=_all/listener"],
    ["/team/settings/sessions", "/home?settings=_all/limits"],
    ["/team/settings/tracing", "/home?settings=_all/diagnostics"],
    ["/team/settings/update", "/home?settings=_all/updates"],
    ["/team/settings/secrets", "/home?settings=_all/keys"],
    ["/team/settings/agent-keys", "/home?settings=_all/keys"],
    ["/team/settings/history", "/home?settings=_all/history"],
    ["/team/settings/nope", "/home?settings=_all/general"],
  ])("sends old hub settings %s to %s", (from, to) => {
    expect(where(from)).toBe(to);
  });

  it("moves an agent section named under the hub to the last-used agent", () => {
    expect(where("/team/settings/providers", "atlas")).toBe("/agent/atlas?settings=atlas/model");
    expect(where("/team/settings/runtime", "atlas")).toBe("/agent/atlas?settings=atlas/runtime");
  });
});

describe("corrections for agents that don't exist", () => {
  const known = new Set(["scout", "atlas"]);

  function correct(url: string): { url: string; notices: string[] } {
    const { location } = read(url);
    const result = correctForAgents(location, known);
    return { url: formatLocation(result.location), notices: result.notices };
  }

  it("leaves a location on agents that exist alone", () => {
    expect(correct("/agent/scout/files?panel=file:a.md&settings=atlas/model")).toEqual({
      url: "/agent/scout/files?panel=file:a.md&settings=atlas/model",
      notices: [],
    });
  });

  it("sends an unknown agent place to Home with a notice, dropping what can't follow", () => {
    expect(correct("/agent/ghost")).toEqual({
      url: "/home",
      notices: [`There's no agent named "ghost".`],
    });
    expect(correct("/agent/ghost/files?panel=file:a.md&settings=scout/model")).toEqual({
      url: "/home?settings=scout/model",
      notices: [`There's no agent named "ghost".`],
    });
  });

  it("sends an unknown settings scope to All agents with a notice", () => {
    expect(correct("/home?settings=ghost/model")).toEqual({
      url: "/home?settings=_all/general",
      notices: [`There's no agent named "ghost".`],
    });
  });

  it("names a missing agent once, however many places it appears", () => {
    expect(correct("/agent/ghost?settings=ghost/model").notices).toEqual([
      `There's no agent named "ghost".`,
    ]);
  });

  it("removes an inbox filter or item for an agent that doesn't exist", () => {
    expect(correct("/inbox?agent=ghost&item=phantom:1")).toEqual({
      url: "/inbox",
      notices: [`There's no agent named "ghost".`, `There's no agent named "phantom".`],
    });
    expect(correct("/inbox?agent=atlas&item=atlas:1").url).toBe("/inbox?agent=atlas&item=atlas:1");
  });

  it("removes a session panel for an agent that doesn't exist", () => {
    expect(correct("/team/workbench?panel=session:ghost:run-1")).toEqual({
      url: "/team/workbench",
      notices: [`There's no agent named "ghost".`],
    });
    expect(correct("/team/workbench?panel=session:atlas:run-1").url).toBe(
      "/team/workbench?panel=session:atlas:run-1",
    );
  });
});

describe("corrections for artifacts that don't exist", () => {
  const artifacts = new Set(["tip-splitter"]);

  it("sends an unknown artifact to the list with a notice", () => {
    const result = correctForArtifacts(read("/team/workbench/ghost").location, artifacts);
    expect(formatLocation(result.location)).toBe("/team/workbench");
    expect(result.notices).toEqual([`There's no workbench page named "ghost".`]);
  });

  it("keeps an artifact that exists, the list, and other places", () => {
    for (const url of ["/team/workbench/tip-splitter", "/team/workbench", "/home"]) {
      const location = read(url).location;
      expect(correctForArtifacts(location, artifacts)).toEqual({ location, notices: [] });
    }
  });

  it("keeps the panel and modal while sending the artifact away", () => {
    const { location } = read("/team/workbench/ghost?panel=session:scout:run-1&settings=_all");
    expect(formatLocation(correctForArtifacts(location, artifacts).location)).toBe(
      "/team/workbench?panel=session:scout:run-1&settings=_all",
    );
  });
});

describe("formatting", () => {
  const locations: AppLocation[] = [
    locationAt(HOME),
    locationAt({
      kind: "inbox",
      agent: "atlas",
      tab: "archived",
      item: { agent: "scout", id: "a:b c" },
    }),
    locationAt({ kind: "chat", agent: "scout" }),
    {
      place: { kind: "files", agent: "scout" },
      panel: { kind: "file", path: "a b/c?d&e.md" },
      settings: null,
    },
    {
      place: { kind: "activity", agent: "scout" },
      panel: session("scout", "run 1:x"),
      settings: null,
    },
    {
      place: { kind: "schedule", agent: "atlas" },
      panel: { kind: "size" },
      settings: { scope: "atlas", section: null },
    },
    {
      place: { kind: "workbench", artifact: "tip-splitter" },
      panel: session("atlas", "r"),
      settings: { scope: "_all", section: "raw" },
    },
    { place: { kind: "shared-files" }, panel: { kind: "file", path: "x.md" }, settings: null },
  ];

  it.each(locations.map((location) => [formatLocation(location), location] as const))(
    "round-trips %s",
    (url, location) => {
      expect(read(url).location).toEqual(location);
      expect(read(url).notices).toEqual([]);
    },
  );

  it("compares locations by what they show", () => {
    expect(
      locationsEqual(read("/agent/scout").location, locationAt({ kind: "chat", agent: "scout" })),
    ).toBe(true);
    expect(
      locationsEqual(read("/agent/scout").location, read("/agent/scout?panel=size").location),
    ).toBe(false);
  });
});
