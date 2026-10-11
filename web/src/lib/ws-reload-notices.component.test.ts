import { beforeEach, describe, expect, it } from "vitest";
import { notifications } from "./notifications.svelte";
import { ws } from "./ws.svelte";

// What the page shows when the agent reloads its settings and no control
// asked for it quietly: the palette's Reload settings, or a settings file
// edited under the page.

beforeEach(() => {
  notifications.clear();
});

describe("the agent reloading its settings", () => {
  it("says so in plain words, and then how it went", () => {
    ws.transport.onMessage?.({ type: "reloading" });
    ws.transport.onMessage?.({ type: "notice", message: "configuration reloaded: models" });

    expect(notifications.history.map((entry) => entry.message).reverse()).toEqual([
      "Reloading settings…",
      "Settings reloaded.",
    ]);
    expect(notifications.history[0]?.details).toBe("configuration reloaded: models");
  });

  it("says when the reload found nothing to apply", () => {
    ws.transport.onMessage?.({
      type: "notice",
      message: "configuration reloaded: no changes detected",
    });
    expect(notifications.history.map((entry) => entry.message)).toEqual([
      "Settings reloaded. Nothing had changed.",
    ]);
  });

  it("raises an error when the new settings couldn't be applied", () => {
    ws.transport.onMessage?.({
      type: "notice",
      message: "config reload failed (keeping current config): invalid TOML",
    });
    expect(notifications.history.map((entry) => [entry.kind, entry.message])).toEqual([
      ["error", "Residuum couldn't apply the new settings and is still using the old ones."],
    ]);
  });

  it("shows any other notice as the agent wrote it", () => {
    ws.transport.onMessage?.({ type: "notice", message: "Reflecting on the week's notes." });
    expect(notifications.history.map((entry) => entry.message)).toEqual([
      "Reflecting on the week's notes.",
    ]);
  });
});
