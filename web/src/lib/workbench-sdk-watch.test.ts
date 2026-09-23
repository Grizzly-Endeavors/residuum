// residuum.watch in the artifact SDK (assets/workbench/sdk.js). The SDK runs
// inside artifact frames, so it is exercised against a stand-in window.

import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

const SDK_SOURCE = readFileSync(
  new URL("../../../assets/workbench/sdk.js", import.meta.url),
  "utf8",
);

interface Frame {
  type: string;
  [key: string]: unknown;
}

interface Sdk {
  watch: (prefix: string, handler: (frame: Frame) => void) => () => void;
  on: (type: string, handler: (frame: Frame) => void) => () => void;
}

interface Loaded {
  residuum: Sdk;
  /** Messages the SDK posted to the web UI. */
  posted: Record<string, unknown>[];
  /** Deliver a bridge message to the SDK. */
  deliver: (data: Record<string, unknown>) => void;
}

function loadSdk(): Loaded {
  const posted: Record<string, unknown>[] = [];
  const parent = {
    postMessage: (message: Record<string, unknown>) => posted.push(message),
  };
  const listeners: ((event: { source: unknown; data: unknown }) => void)[] = [];
  const win: Record<string, unknown> = {
    parent,
    addEventListener: (type: string, fn: (event: { source: unknown; data: unknown }) => void) => {
      if (type === "message") listeners.push(fn);
    },
  };
  runInNewContext(SDK_SOURCE, {
    window: win,
    console,
    __RESIDUUM_ARTIFACT__: "chart",
    __RESIDUUM_VERSION__: "test",
    __RESIDUUM_FEATURES__: ["workspace-watch"],
  });
  return {
    residuum: win.residuum as Sdk,
    posted,
    deliver: (data) => {
      for (const fn of listeners)
        fn({ source: parent, data: { tag: "residuum-workbench", ...data } });
    },
  };
}

const changed = (...paths: string[]): Frame => ({
  type: "workspace_changed",
  changes: paths.map((path) => ({ path, kind: "modified" })),
});

describe("residuum.watch", () => {
  it("announces a new document before anything else", () => {
    const sdk = loadSdk();
    expect(sdk.posted).toEqual([{ tag: "residuum-workbench", kind: "ready" }]);
  });

  it("sends the union of watched prefixes, and again when one unsubscribes", () => {
    const sdk = loadSdk();
    const stopWiki = sdk.residuum.watch("wiki/", () => {});
    sdk.residuum.watch("inbox/user", () => {});
    stopWiki();
    const watches = sdk.posted.filter((m) => m.kind === "watch").map((m) => m.prefixes);
    expect(watches).toEqual([["wiki"], ["wiki", "inbox/user"], ["inbox/user"]]);
  });

  it("refuses a prefix outside the workspace", () => {
    const sdk = loadSdk();
    // The SDK runs in its own realm, so its TypeError is matched by message.
    expect(() => sdk.residuum.watch("../secrets", () => {})).toThrow(/leaves the workspace/);
    expect(() => sdk.residuum.watch("/etc", () => {})).toThrow(/relative to the workspace/);
    expect(sdk.posted.filter((m) => m.kind === "watch")).toEqual([]);
  });

  it("gives each handler only the changes under its own prefix", () => {
    const sdk = loadSdk();
    const wiki: Frame[] = [];
    const notes: Frame[] = [];
    sdk.residuum.watch("wiki", (frame) => wiki.push(frame));
    sdk.residuum.watch("notes", (frame) => notes.push(frame));

    sdk.deliver({ kind: "event", frame: changed("wiki/a.md", "wikipedia/b.md", "wiki/c.md") });
    expect(wiki).toEqual([changed("wiki/a.md", "wiki/c.md")]);
    expect(notes).toEqual([]);
  });

  it("gives every handler a resync", () => {
    const sdk = loadSdk();
    const seen: Frame[] = [];
    sdk.residuum.watch("wiki", (frame) => seen.push(frame));
    sdk.residuum.watch("notes", (frame) => seen.push(frame));
    const resync = { type: "workspace_resync", reason: "reconnected" };
    sdk.deliver({ kind: "event", frame: resync });
    expect(seen).toEqual([resync, resync]);
  });

  it("stops calling a handler once unsubscribed", () => {
    const sdk = loadSdk();
    const seen: Frame[] = [];
    const stop = sdk.residuum.watch("wiki", (frame) => seen.push(frame));
    stop();
    sdk.deliver({ kind: "event", frame: changed("wiki/a.md") });
    expect(seen).toEqual([]);
  });

  it("passes bridge-generated connection frames to residuum.on", () => {
    const sdk = loadSdk();
    const seen: Frame[] = [];
    sdk.residuum.on("connection", (frame) => seen.push(frame));
    sdk.deliver({ kind: "event", frame: { type: "connection", state: "disconnected" } });
    expect(seen).toEqual([{ type: "connection", state: "disconnected" }]);
  });
});
