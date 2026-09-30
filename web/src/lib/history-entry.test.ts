import { describe, expect, it } from "vitest";
import {
  entryAfterPush,
  entryAfterReplace,
  overlayEntryAfter,
  readEntry,
  sameEntry,
  stepsToClose,
  withoutOverlay,
} from "./history-entry";
import { HOME, locationAt, type AppLocation } from "./routes";

const plain = locationAt({ kind: "chat", agent: "scout" });
const withSettings: AppLocation = {
  ...plain,
  settings: { scope: "scout", section: "model" },
};
const withPanel: AppLocation = { ...plain, panel: { kind: "size" } };
const withBoth: AppLocation = { ...withPanel, settings: { scope: "scout", section: null } };

describe("reading an entry's state", () => {
  it("reads what the router wrote", () => {
    expect(readEntry({ idx: 3, settings: 2, panel: 1, overlay: "overlay-1" })).toEqual({
      idx: 3,
      settings: 2,
      panel: 1,
      overlay: "overlay-1",
    });
  });

  it("doesn't know state it didn't write", () => {
    expect(readEntry(null)).toBeNull();
    expect(readEntry(undefined)).toBeNull();
    expect(readEntry("x")).toBeNull();
    expect(readEntry({})).toBeNull();
    expect(readEntry({ idx: "3" })).toBeNull();
  });

  it("ignores marks of the wrong type", () => {
    expect(readEntry({ idx: 1, settings: "x", panel: null, overlay: 3 })).toEqual({ idx: 1 });
  });
});

describe("marks on a push", () => {
  it("counts every push one past the entry before it", () => {
    expect(entryAfterPush({ idx: 4 }, plain, HOME_LOCATION).idx).toBe(5);
  });

  it("marks the modal's opener when the push opens it", () => {
    expect(entryAfterPush({ idx: 2 }, plain, withSettings)).toEqual({ idx: 3, settings: 3 });
  });

  it("keeps the opener when the modal was already open and this page pushed its entry", () => {
    expect(entryAfterPush({ idx: 3, settings: 3 }, withSettings, withSettings)).toEqual({
      idx: 4,
      settings: 3,
    });
  });

  it("has no opener when the modal was already open from a link", () => {
    expect(entryAfterPush({ idx: 0 }, withSettings, withSettings)).toEqual({ idx: 1 });
  });

  it("marks the panel and the modal separately", () => {
    expect(entryAfterPush({ idx: 1, panel: 1 }, withPanel, withBoth)).toEqual({
      idx: 2,
      panel: 1,
      settings: 2,
    });
  });

  it("carries no mark for what the new location doesn't have", () => {
    expect(entryAfterPush({ idx: 3, settings: 3, panel: 2 }, withBoth, plain)).toEqual({ idx: 4 });
  });
});

describe("marks when the place changes", () => {
  const files = { ...withSettings, place: { kind: "files", agent: "scout" } } as AppLocation;

  it("has none on a push that opens the modal along with another place", () => {
    expect(entryAfterPush({ idx: 2 }, plain, files)).toEqual({ idx: 3 });
    expect(entryAfterPush({ idx: 2 }, withPanel, { ...files, panel: { kind: "size" } })).toEqual({
      idx: 3,
    });
  });

  it("drops them on a push to another place", () => {
    expect(entryAfterPush({ idx: 3, settings: 3 }, withSettings, files)).toEqual({ idx: 4 });
  });

  it("drops them on a replace to another place", () => {
    expect(entryAfterReplace({ idx: 3, settings: 3 }, withSettings, files)).toEqual({ idx: 3 });
  });
});

describe("marks on a replace", () => {
  it("keeps the opener of what stays open", () => {
    expect(entryAfterReplace({ idx: 3, settings: 3 }, withSettings, withSettings)).toEqual({
      idx: 3,
      settings: 3,
    });
  });

  it("drops the opener of what was closed", () => {
    expect(entryAfterReplace({ idx: 3, settings: 3 }, withSettings, plain)).toEqual({ idx: 3 });
  });

  it("gives what a replace opens no opener, since this page didn't push an entry for it", () => {
    expect(entryAfterReplace({ idx: 3 }, plain, withSettings)).toEqual({ idx: 3 });
  });

  it("keeps the overlay mark", () => {
    expect(entryAfterReplace({ idx: 3, overlay: "overlay-1" }, plain, plain)).toEqual({
      idx: 3,
      overlay: "overlay-1",
    });
  });
});

describe("overlay entries", () => {
  it("is the next entry, with the marks below it and its own", () => {
    expect(overlayEntryAfter({ idx: 3, settings: 3 }, "overlay-2")).toEqual({
      idx: 4,
      settings: 3,
      overlay: "overlay-2",
    });
  });

  it("drops the mark when the overlay is gone", () => {
    expect(withoutOverlay({ idx: 4, settings: 3, overlay: "overlay-2" })).toEqual({
      idx: 4,
      settings: 3,
    });
  });

  it("compares entries by every mark", () => {
    expect(sameEntry({ idx: 1, panel: 1 }, { idx: 1, panel: 1 })).toBe(true);
    expect(sameEntry({ idx: 1, panel: 1 }, { idx: 1 })).toBe(false);
    expect(sameEntry({ idx: 1, overlay: "a" }, { idx: 1 })).toBe(false);
    expect(sameEntry(null, { idx: 1 })).toBe(false);
  });
});

describe("closing", () => {
  it("goes back to before the opener", () => {
    expect(stepsToClose({ idx: 3 }, 3)).toBe(1);
    expect(stepsToClose({ idx: 5 }, 3)).toBe(3);
  });

  it("replaces when this page didn't push the opener", () => {
    expect(stepsToClose({ idx: 5 }, undefined)).toBeNull();
  });
});

const HOME_LOCATION = locationAt(HOME);
