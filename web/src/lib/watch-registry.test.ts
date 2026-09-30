import { describe, expect, it } from "vitest";
import { WatchRegistry, type WatchHandler } from "./watch-registry";
import { normalizeTeamWatchPrefix, normalizeWatchPrefix } from "./workspace-watch";
import type { WorkspaceChange } from "./types";

interface Setup {
  registry: WatchRegistry;
  /** Every watch set the registry sent, in order. */
  sent: string[][];
}

function setup(options: { team?: boolean } = {}): Setup {
  const sent: string[][] = [];
  const registry = new WatchRegistry({
    send: (prefixes) => sent.push(prefixes),
    normalize: options.team ? normalizeTeamWatchPrefix : normalizeWatchPrefix,
    refusal: (prefix) => `refused ${prefix}`,
  });
  return { registry, sent };
}

/** A handler that records what it hears. */
function listener(): {
  handler: WatchHandler;
  changes: string[][];
  resyncs: string[];
  unavailable: string[];
} {
  const changes: string[][] = [];
  const resyncs: string[] = [];
  const unavailable: string[] = [];
  return {
    changes,
    resyncs,
    unavailable,
    handler: {
      changed: (batch) => changes.push(batch.map((c) => c.path)),
      resync: (reason) => resyncs.push(reason),
      unavailable: (message) => unavailable.push(message),
    },
  };
}

const modified = (...paths: string[]): WorkspaceChange[] =>
  paths.map((path) => ({ path, kind: "modified" as const }));

describe("WatchRegistry prefixes", () => {
  it("merges two owners' prefixes into one sorted union", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    const b = registry.register(listener().handler);

    a.set(["wiki", "inbox/user"]);
    b.set(["notes", "wiki"]);

    expect(registry.prefixes).toEqual(["inbox/user", "notes", "wiki"]);
    expect(sent).toEqual([
      ["inbox/user", "wiki"],
      ["inbox/user", "notes", "wiki"],
    ]);
  });

  it("normalizes each owner's prefixes and sends only when the union changes", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    const b = registry.register(listener().handler);

    a.set(["wiki/", "./wiki"]);
    b.set(["wiki"]);
    a.set(["wiki"]);
    expect(a.prefixes).toEqual(["wiki"]);
    expect(sent).toEqual([["wiki"]]);
  });

  it("leaves the other owner's watches in place when one changes its own or unregisters", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    const b = registry.register(listener().handler);
    a.set(["wiki"]);
    b.set(["notes"]);

    a.set([]);
    expect(sent.at(-1)).toEqual(["notes"]);

    a.set(["wiki"]);
    a.release();
    expect(sent.at(-1)).toEqual(["notes"]);
    expect(registry.prefixes).toEqual(["notes"]);

    b.release();
    expect(sent.at(-1)).toEqual([]);
  });

  it("keeps a prefix two owners share until both let go", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    const b = registry.register(listener().handler);
    a.set(["wiki"]);
    b.set(["wiki"]);
    a.release();
    expect(registry.prefixes).toEqual(["wiki"]);
    expect(sent).toEqual([["wiki"]]);
  });

  it("refuses a prefix the socket would refuse, and leaves the owner's prefixes as they were", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    a.set(["wiki"]);

    expect(() => {
      a.set(["notes", "../secrets"]);
    }).toThrow(new TypeError("refused ../secrets"));
    expect(a.prefixes).toEqual(["wiki"]);
    expect(sent).toEqual([["wiki"]]);
  });

  it("spells team prefixes as the hub does", () => {
    const { registry, sent } = setup({ team: true });
    registry.connected();
    const a = registry.register(listener().handler);
    a.set(["team", "team//workbench/./chart/"]);
    expect(sent).toEqual([["team", "team/workbench/chart"]]);
    expect(() => {
      a.set(["wiki"]);
    }).toThrow(TypeError);
  });

  it("ignores a released owner", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    a.release();
    a.set(["wiki"]);
    a.release();
    expect(sent).toEqual([]);
    expect(registry.prefixes).toEqual([]);
  });
});

describe("WatchRegistry connection", () => {
  it("sends nothing before the socket is open, then the union once it opens", () => {
    const { registry, sent } = setup();
    const a = registry.register(listener().handler);
    a.set(["wiki"]);
    expect(sent).toEqual([]);

    registry.connected();
    expect(sent).toEqual([["wiki"]]);
  });

  it("sends the union again on every reconnect, since a new connection watches nothing", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    const b = registry.register(listener().handler);
    a.set(["wiki"]);
    b.set(["notes"]);
    sent.length = 0;

    registry.disconnected();
    registry.connected();
    registry.disconnected();
    registry.connected();
    expect(sent).toEqual([
      ["notes", "wiki"],
      ["notes", "wiki"],
    ]);
  });

  it("sends nothing on a connection when nothing is watched", () => {
    const { registry, sent } = setup();
    registry.connected();
    registry.disconnected();
    registry.connected();
    expect(sent).toEqual([]);
  });

  it("holds changes made while disconnected until the socket is back", () => {
    const { registry, sent } = setup();
    registry.connected();
    const a = registry.register(listener().handler);
    a.set(["wiki"]);
    registry.disconnected();
    a.set(["wiki", "notes"]);
    expect(sent).toEqual([["wiki"]]);

    registry.connected();
    expect(sent).toEqual([["wiki"], ["notes", "wiki"]]);
  });
});

describe("WatchRegistry bound agent", () => {
  it("sends the prefixes that still apply to the new agent's connection", () => {
    const { registry, sent } = setup();
    registry.bind("scout");
    registry.connected();
    const followsAgent = registry.register(listener().handler);
    const scoutFiles = registry.register(listener().handler, { agent: "scout" });
    const atlasFiles = registry.register(listener().handler, { agent: "atlas" });
    followsAgent.set(["config"]);
    scoutFiles.set([""]);
    atlasFiles.set(["memory"]);
    expect(sent.at(-1)).toEqual(["", "config"]);

    registry.bind("atlas");
    expect(sent.at(-1)).toEqual(["", "config"]);
    const before = sent.length;
    registry.connected();
    expect(sent).toHaveLength(before + 1);
    expect(sent.at(-1)).toEqual(["config", "memory"]);
    expect(registry.prefixes).toEqual(["config", "memory"]);
  });

  it("brings an agent's own watches back when it is bound again", () => {
    const { registry, sent } = setup();
    registry.bind("scout");
    registry.connected();
    registry.register(listener().handler, { agent: "scout" }).set(["memory"]);

    registry.bind("atlas");
    registry.connected();
    expect(sent.at(-1)).toEqual(["memory"]);
    registry.bind("scout");
    registry.connected();
    expect(sent.at(-1)).toEqual(["memory"]);
    expect(sent).toHaveLength(2);
    registry.bind("atlas");
    expect(registry.prefixes).toEqual([]);
  });

  it("does not hear the changes of a watch tied to another agent", () => {
    const { registry } = setup();
    registry.bind("atlas");
    const scout = listener();
    const atlas = listener();
    registry.register(scout.handler, { agent: "scout" }).set(["memory"]);
    registry.register(atlas.handler, { agent: "atlas" }).set(["memory"]);

    registry.handleFrame({ type: "workspace_changed", changes: modified("memory/a.md") });
    registry.handleFrame({ type: "workspace_resync", reason: "overflow" });
    expect(scout.changes).toEqual([]);
    expect(scout.resyncs).toEqual([]);
    expect(atlas.changes).toEqual([["memory/a.md"]]);
    expect(atlas.resyncs).toEqual(["overflow"]);
  });
});

describe("WatchRegistry delivery", () => {
  it("delivers each owner only the changes under its own prefixes", () => {
    const { registry } = setup();
    registry.connected();
    const wiki = listener();
    const inbox = listener();
    const idle = listener();
    registry.register(wiki.handler).set(["wiki"]);
    registry.register(inbox.handler).set(["inbox/user", "wiki/people"]);
    registry.register(idle.handler);

    registry.handleFrame({
      type: "workspace_changed",
      changes: modified("wiki/a.md", "wikipedia/b.md", "inbox/user/x.md", "wiki/people/sam.md"),
    });

    expect(wiki.changes).toEqual([["wiki/a.md", "wiki/people/sam.md"]]);
    expect(inbox.changes).toEqual([["inbox/user/x.md", "wiki/people/sam.md"]]);
    expect(idle.changes).toEqual([]);
  });

  it("delivers nothing to an owner none of whose prefixes a batch concerns", () => {
    const { registry } = setup();
    const wiki = listener();
    registry.register(wiki.handler).set(["wiki"]);
    registry.handleFrame({ type: "workspace_changed", changes: modified("notes/a.md") });
    expect(wiki.changes).toEqual([]);
  });

  it("matches the gateway's rule: a folder containing a prefix concerns it", () => {
    const { registry } = setup();
    const alpha = listener();
    registry.register(alpha.handler).set(["projects/alpha"]);
    registry.handleFrame({ type: "workspace_changed", changes: modified("projects") });
    expect(alpha.changes).toEqual([["projects"]]);
  });

  it("delivers a resync and an unavailable frame to every owner", () => {
    const { registry } = setup();
    const a = listener();
    const b = listener();
    const notWatching = listener();
    registry.register(a.handler).set(["wiki"]);
    registry.register(b.handler).set(["notes"]);
    registry.register(notWatching.handler);

    registry.handleFrame({ type: "workspace_resync", reason: "watcher_restarted" });
    registry.handleFrame({ type: "workspace_watch_unavailable", message: "off" });

    for (const owner of [a, b, notWatching]) {
      expect(owner.resyncs).toEqual(["watcher_restarted"]);
      expect(owner.unavailable).toEqual(["off"]);
    }
  });

  it("stops delivering to an owner that has released", () => {
    const { registry } = setup();
    const a = listener();
    const b = listener();
    const ownerA = registry.register(a.handler);
    ownerA.set(["wiki"]);
    registry.register(b.handler).set(["wiki"]);
    ownerA.release();

    registry.handleFrame({ type: "workspace_changed", changes: modified("wiki/a.md") });
    registry.handleFrame({ type: "workspace_resync", reason: "overflow" });
    expect(a.changes).toEqual([]);
    expect(a.resyncs).toEqual([]);
    expect(b.changes).toEqual([["wiki/a.md"]]);
    expect(b.resyncs).toEqual(["overflow"]);
  });

  it("does not call an owner that an earlier handler released while hearing the same frame", () => {
    const { registry } = setup();
    const second = listener();
    let secondOwner: ReturnType<WatchRegistry["register"]> | null = null;
    registry
      .register({
        changed: () => {
          secondOwner?.release();
        },
      })
      .set(["wiki"]);
    secondOwner = registry.register(second.handler);
    secondOwner.set(["wiki"]);

    registry.handleFrame({ type: "workspace_changed", changes: modified("wiki/a.md") });
    expect(second.changes).toEqual([]);
  });

  it("ignores frames that are not the change feed", () => {
    const { registry } = setup();
    const a = listener();
    registry.register(a.handler).set([""]);
    registry.handleFrame({ type: "pong" });
    expect(a.changes).toEqual([]);
    expect(a.resyncs).toEqual([]);
  });

  it("lets every owner hear a frame even when one handler throws, then reports the failure", () => {
    const { registry } = setup();
    const after = listener();
    registry
      .register({
        changed: () => {
          throw new Error("boom");
        },
      })
      .set(["wiki"]);
    registry.register(after.handler).set(["wiki"]);

    expect(() => {
      registry.handleFrame({ type: "workspace_changed", changes: modified("wiki/a.md") });
    }).toThrow("boom");
    expect(after.changes).toEqual([["wiki/a.md"]]);
  });

  it("reports several failures together", () => {
    const { registry } = setup();
    for (const message of ["first", "second"]) {
      registry
        .register({
          changed: () => {
            throw new Error(message);
          },
        })
        .set(["wiki"]);
    }
    expect(() => {
      registry.handleFrame({ type: "workspace_changed", changes: modified("wiki/a.md") });
    }).toThrow(AggregateError);
  });
});
