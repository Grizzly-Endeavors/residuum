import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  hubReloadNotice,
  hubReloadOutcome,
  noticeFrameNotice,
  quietReloads,
  reloadingNotice,
} from "./reload-notices";

const REPORT = "configuration reloaded: models";
const NOTHING_CHANGED = "configuration reloaded: no changes detected";
const FAILED = "config reload failed (keeping current config): invalid TOML at line 4";

const reloaded = { message: REPORT, reload: "reloaded", reload_of: "agent_config" } as const;
const unchanged = {
  message: NOTHING_CHANGED,
  reload: "unchanged",
  reload_of: "agent_config",
} as const;
const failed = { message: FAILED, reload: "failed", reload_of: "agent_config" } as const;

const HUB_REPORT = "hub configuration reloaded: timezone";
const HUB_FAILED = "hub config reload failed (keeping current hub config): bad toml";
const WORKSPACE_REPORT = "workspace configuration reloaded";

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

  it("words each outcome itself, keeping the agent's own words behind the detail", () => {
    expect(noticeFrameNotice(reloaded)).toEqual({
      kind: "notice",
      message: "Settings reloaded.",
      details: REPORT,
    });
    expect(noticeFrameNotice(unchanged)).toEqual({
      kind: "notice",
      message: "Settings reloaded. Nothing had changed.",
      details: NOTHING_CHANGED,
    });
    expect(noticeFrameNotice(failed)).toEqual({
      kind: "error",
      message: "Residuum couldn't apply the new settings and is still using the old ones.",
      details: FAILED,
    });
  });

  it("goes by the outcome, not by what the text says", () => {
    expect(noticeFrameNotice({ message: "anything at all", reload: "reloaded" })).toMatchObject({
      message: "Settings reloaded.",
    });
    // Text that reads like a reload report but carries no outcome is any other notice.
    expect(noticeFrameNotice({ message: REPORT })).toEqual({ kind: "notice", message: REPORT });
  });

  it("names the agent's files when the workspace reloaded", () => {
    expect(
      noticeFrameNotice({ message: WORKSPACE_REPORT, reload: "reloaded", reload_of: "workspace" }),
    ).toEqual({ kind: "notice", message: "Agent files reloaded.", details: WORKSPACE_REPORT });
  });

  it("leaves the agent's reload of the hub's settings to the hub's own report", () => {
    expect(
      noticeFrameNotice({ message: HUB_REPORT, reload: "reloaded", reload_of: "hub_config" }),
    ).toBeNull();
    expect(
      noticeFrameNotice({
        message: "hub configuration reloaded: no changes detected",
        reload: "unchanged",
        reload_of: "hub_config",
      }),
    ).toBeNull();
  });

  it("still shows an agent failing to apply the hub's settings, as an error", () => {
    expect(
      noticeFrameNotice({ message: HUB_FAILED, reload: "failed", reload_of: "hub_config" }),
    ).toEqual({
      kind: "error",
      message:
        "This agent couldn't apply the new settings for all agents and is still using the old ones.",
      details: HUB_FAILED,
    });
  });

  it("passes any other notice along as it came", () => {
    expect(noticeFrameNotice({ message: "Reflecting on the week's notes." })).toEqual({
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

  it("says nothing of its report, nor of the watcher's second pass that finds nothing new", () => {
    quietReloads.expect();
    expect(noticeFrameNotice(reloaded)).toBeNull();
    vi.advanceTimersByTime(3_000);
    expect(noticeFrameNotice(unchanged)).toBeNull();
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
    expect(noticeFrameNotice(reloaded)).toBeNull();
    expect(noticeFrameNotice(failed)).toEqual({
      kind: "error",
      message: "Residuum couldn't apply the new settings and is still using the old ones.",
      details: FAILED,
    });
  });

  it("never hides another notice", () => {
    quietReloads.expect();
    expect(noticeFrameNotice({ message: "skipped provider openai: no key" })).toMatchObject({
      message: "skipped provider openai: no key",
    });
  });

  it("stops being quiet once its window has passed", () => {
    quietReloads.expect();
    vi.advanceTimersByTime(16_000);
    expect(noticeFrameNotice(reloaded)).toMatchObject({ message: "Settings reloaded." });
    // A request whose answer never came doesn't hide a later reload's.
    expect(reloadingNotice()).toMatchObject({ message: "Reloading settings…" });
  });
});

describe("the hub's report of a reload of the settings for all agents", () => {
  const changedFrame = { ok: true, changed: true, message: "hub configuration reloaded: timezone" };
  const unchangedFrame = { ok: true, changed: false, message: null };
  const failedFrame = {
    ok: false,
    changed: false,
    message: "hub config reload failed (keeping current hub config): bad toml",
  };

  it("reads its outcome from whether it loaded and whether anything changed", () => {
    expect(hubReloadOutcome(changedFrame)).toBe("reloaded");
    expect(hubReloadOutcome(unchangedFrame)).toBe("unchanged");
    expect(hubReloadOutcome(failedFrame)).toBe("failed");
  });

  it("says a change was applied, in plain words, with the hub's own behind the detail", () => {
    expect(hubReloadNotice(changedFrame)).toEqual({
      kind: "notice",
      message: "Settings for all agents reloaded.",
      details: "hub configuration reloaded: timezone",
    });
  });

  it("says nothing when the reload found nothing new", () => {
    expect(hubReloadNotice(unchangedFrame)).toBeNull();
  });

  it("raises an error when the new settings couldn't be applied, even during a quiet request", () => {
    quietReloads.expect();
    expect(hubReloadNotice(failedFrame)).toEqual({
      kind: "error",
      message:
        "Residuum couldn't apply the new settings for all agents and is still using the old ones.",
      details: "hub config reload failed (keeping current hub config): bad toml",
    });
  });
});
