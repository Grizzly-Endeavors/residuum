import { describe, expect, it } from "vitest";
import { hubNoticeText } from "./hub-notices";

describe("hubNoticeText", () => {
  it("shows a notice without a kind as the hub wrote it", () => {
    expect(hubNoticeText(undefined, "Team wiki search is text-only.")).toEqual({
      message: "Team wiki search is text-only.",
    });
  });

  it("words each kind itself, keeping the hub's text behind the detail", () => {
    const rebind = "gateway rebind failed (0.0.0.0:7701): in use — keeping the current server";
    expect(hubNoticeText("gateway_rebind_failed", rebind)).toEqual({
      message:
        "Residuum couldn't move to the new address in its settings and is still reachable at the old one.",
      details: rebind,
    });
    const limit =
      "background.max_concurrent changed in hub/config.toml — this takes effect on the next restart, not immediately.";
    expect(hubNoticeText("background_limit_after_restart", limit)).toEqual({
      message:
        "The new limit on background sessions running at once takes effect when Residuum restarts.",
      details: limit,
    });
  });
});
