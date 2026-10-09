import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { noticeFrameNotice, quietReloads, reloadingNotice } from "./reload-notices";

const REPORT = "configuration reloaded: models";
const NOTHING_CHANGED = "configuration reloaded: no changes detected";
const FAILED = "config reload failed (keeping current config): invalid TOML at line 4";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-09T12:00:00Z"));
  // Whatever an earlier test left open has ended.
  vi.advanceTimersByTime(60_000);
  quietReloads.takeReloading();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("a reload nobody asked to keep quiet", () => {
  it("says it is reloading, in plain words", () => {
    expect(reloadingNotice()).toEqual({ kind: "system", message: "Reloading settings…" });
  });

  it("puts the agent's success report in plain words, keeping its own behind the detail", () => {
    expect(noticeFrameNotice(REPORT)).toEqual({
      kind: "notice",
      message: "Settings reloaded.",
      details: REPORT,
    });
    expect(noticeFrameNotice(NOTHING_CHANGED)).toEqual({
      kind: "notice",
      message: "Settings reloaded. Nothing had changed.",
      details: NOTHING_CHANGED,
    });
    expect(noticeFrameNotice("Configuration reloaded successfully.")).toMatchObject({
      message: "Settings reloaded.",
    });
  });

  it("puts a hub reload report in plain words too", () => {
    expect(noticeFrameNotice("hub configuration reloaded: timezone")).toMatchObject({
      message: "Settings reloaded.",
    });
  });

  it("passes any other notice along as it came", () => {
    expect(noticeFrameNotice("Reflecting on the week's notes.")).toEqual({
      kind: "notice",
      message: "Reflecting on the week's notes.",
    });
  });
});

describe("a reload a control asked for quietly", () => {
  it("says nothing of the reload starting, once", () => {
    quietReloads.expect();
    expect(reloadingNotice()).toBeNull();
    expect(reloadingNotice()).toMatchObject({ message: "Reloading settings…" });
  });

  it("says nothing of its success reports, including the watcher's second pass", () => {
    quietReloads.expect();
    expect(noticeFrameNotice(REPORT)).toBeNull();
    vi.advanceTimersByTime(3_000);
    expect(noticeFrameNotice(NOTHING_CHANGED)).toBeNull();
  });

  it("keeps a second quiet request quiet when it follows before the first is answered", () => {
    quietReloads.expect();
    quietReloads.expect();
    expect(reloadingNotice()).toBeNull();
    expect(reloadingNotice()).toBeNull();
    expect(reloadingNotice()).not.toBeNull();
  });

  it("never hides a failed reload, and shows it as an error in plain words", () => {
    quietReloads.expect();
    expect(noticeFrameNotice(FAILED)).toEqual({
      kind: "error",
      message: "Residuum couldn't apply the new settings and is still using the old ones.",
      details: FAILED,
    });
  });

  it("never hides another notice", () => {
    quietReloads.expect();
    expect(noticeFrameNotice("skipped provider openai: no key")).toMatchObject({
      message: "skipped provider openai: no key",
    });
  });

  it("stops being quiet once its window has passed", () => {
    quietReloads.expect();
    vi.advanceTimersByTime(16_000);
    expect(noticeFrameNotice(REPORT)).toMatchObject({ message: "Settings reloaded." });
    // A request whose answer never came doesn't hide a later reload's.
    expect(reloadingNotice()).toMatchObject({ message: "Reloading settings…" });
  });
});
