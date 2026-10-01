import { afterEach, describe, expect, it, vi } from "vitest";
import { fileName, fileSourceFor } from "./panel-file";
import { panelLayout } from "./panel-frame";
import {
  clampPanelWidth,
  PANEL_DEFAULT_WIDTH,
  PANEL_LARGE_STEP,
  PANEL_MIN_WIDTH,
  PANEL_STEP,
  panelMaxWidth,
  panelWidthForKey,
  readPanelWidth,
  savePanelWidth,
} from "./panel-width";

/** A `localStorage` that works, which the Node test environment lacks. */
function stubStorage(): Map<string, string> {
  const items = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => {
      items.set(key, value);
    },
  });
  return items;
}

/** A `localStorage` that refuses every call, as a private window or blocked site data does. */
function stubBlockedStorage(): void {
  const refuse = (): never => {
    throw new DOMException("The operation is insecure.", "SecurityError");
  };
  vi.stubGlobal("localStorage", { getItem: refuse, setItem: refuse });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the panel's layout", () => {
  it("is a sheet on phones, beside the main region when wide, and over it between", () => {
    expect(panelLayout({ phone: true, wide: false })).toBe("phone");
    expect(panelLayout({ phone: false, wide: true })).toBe("wide");
    expect(panelLayout({ phone: false, wide: false })).toBe("medium");
  });
});

describe("the panel's width", () => {
  it("goes no wider than half the viewport, and no narrower than the minimum", () => {
    expect(panelMaxWidth(1440)).toBe(720);
    expect(panelMaxWidth(1301)).toBe(650);
    expect(clampPanelWidth(900, 1440)).toBe(720);
    expect(clampPanelWidth(200, 1440)).toBe(PANEL_MIN_WIDTH);
    expect(clampPanelWidth(512.6, 1440)).toBe(513);
  });

  it("keeps the minimum when half the viewport is narrower than it", () => {
    expect(panelMaxWidth(600)).toBe(PANEL_MIN_WIDTH);
    expect(clampPanelWidth(500, 600)).toBe(PANEL_MIN_WIDTH);
  });

  it("falls back to the default for a width that isn't a number", () => {
    expect(clampPanelWidth(Number.NaN, 1440)).toBe(PANEL_DEFAULT_WIDTH);
  });

  it("follows the resize handle's keys: Left widens, Right narrows, Home and End go to the ends", () => {
    expect(panelWidthForKey("ArrowLeft", false, 440, 1440)).toBe(440 + PANEL_STEP);
    expect(panelWidthForKey("ArrowRight", false, 440, 1440)).toBe(440 - PANEL_STEP);
    expect(panelWidthForKey("ArrowLeft", true, 440, 1440)).toBe(440 + PANEL_LARGE_STEP);
    expect(panelWidthForKey("ArrowRight", false, PANEL_MIN_WIDTH, 1440)).toBe(PANEL_MIN_WIDTH);
    expect(panelWidthForKey("ArrowLeft", false, 720, 1440)).toBe(720);
    expect(panelWidthForKey("Home", false, 600, 1440)).toBe(PANEL_MIN_WIDTH);
    expect(panelWidthForKey("End", false, 440, 1440)).toBe(720);
    expect(panelWidthForKey("Enter", false, 440, 1440)).toBeNull();
  });
});

describe("the remembered width", () => {
  it("reads back what was saved, in whole pixels", () => {
    stubStorage();
    expect(readPanelWidth()).toBeNull();
    savePanelWidth(512.4);
    expect(readPanelWidth()).toBe(512);
  });

  it("ignores a stored value that isn't a width", () => {
    const items = stubStorage();
    items.set("residuum-panel-width", "wide");
    expect(readPanelWidth()).toBeNull();
    items.set("residuum-panel-width", "-20");
    expect(readPanelWidth()).toBeNull();
  });

  it("does without storage when the browser blocks it", () => {
    stubBlockedStorage();
    expect(readPanelWidth()).toBeNull();
    expect(() => {
      savePanelWidth(500);
    }).not.toThrow();
  });
});

describe("a file panel's tree", () => {
  it("is the agent's workspace on its places, and the team's folder on Shared files", () => {
    expect(fileSourceFor({ kind: "chat", agent: "atlas" })).toEqual({
      agent: "atlas",
      scope: "agent",
    });
    expect(fileSourceFor({ kind: "files", agent: "scout" })).toEqual({
      agent: "scout",
      scope: "agent",
    });
    expect(fileSourceFor({ kind: "shared-files" })).toEqual({ agent: null, scope: "team" });
  });

  it("is none on places that can't show a file", () => {
    expect(fileSourceFor({ kind: "home" })).toBeNull();
    expect(fileSourceFor({ kind: "workbench", artifact: null })).toBeNull();
  });

  it("titles a file by its last segment", () => {
    expect(fileName("team/wiki/index.md")).toBe("index.md");
    expect(fileName("SOUL.md")).toBe("SOUL.md");
    expect(fileName("notes/")).toBe("notes");
  });
});
