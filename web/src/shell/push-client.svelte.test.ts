// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { router } from "../lib/router.svelte";
import { INBOX } from "../lib/routes";
import { openNotificationTarget } from "./push-client.svelte";

describe("following a notification's target", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each([
    ["the current form", "/inbox?thread=atlas%2Fuser%2Fnote-1"],
    ["the form older notifications carry", "/inbox?item=atlas:note-1"],
  ])("opens the thread in %s", async (_name, target) => {
    const open = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    await expect(openNotificationTarget(target)).resolves.toBe(true);
    expect(open).toHaveBeenCalledWith(
      { ...INBOX, thread: "atlas/user/note-1" },
      { panel: undefined, settings: undefined },
    );
  });

  it("opens a New message with its recipients, and a file beside a thread", async () => {
    const open = vi.spyOn(router, "openPlace").mockResolvedValue(true);
    await openNotificationTarget("/inbox?compose=1&to=atlas");
    expect(open).toHaveBeenLastCalledWith(
      { ...INBOX, compose: { to: ["atlas"] } },
      { panel: undefined, settings: undefined },
    );
    await openNotificationTarget(
      "/inbox?thread=atlas%2Fuser%2Fa&panel=agent-file:atlas:inbox/user/a/report.md",
    );
    expect(open).toHaveBeenLastCalledWith(
      { ...INBOX, thread: "atlas/user/a" },
      {
        panel: { kind: "file", path: "inbox/user/a/report.md", agent: "atlas" },
        settings: undefined,
      },
    );
  });
});
