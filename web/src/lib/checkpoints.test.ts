import { describe, expect, it } from "vitest";
import {
  diffLineKind,
  encryptedRestoreHint,
  formatBytes,
  historyRepos,
  isEncryptedConfigFile,
  triggerLabel,
  undoReport,
} from "./checkpoints";
import type { UndoOutcome } from "./types";

describe("triggerLabel", () => {
  it("labels every trigger a checkpoint can carry", () => {
    expect(triggerLabel("turn_start")).toBe("Turn start");
    expect(triggerLabel("turn_end")).toBe("Turn end");
    expect(triggerLabel("pre_action")).toBe("Before action");
    expect(triggerLabel("pre_config_write")).toBe("Before save");
    expect(triggerLabel("restore")).toBe("Restore");
    expect(triggerLabel("undo")).toBe("Undo");
  });
});

describe("formatBytes", () => {
  it("formats bytes, kilobytes, and megabytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });
});

describe("isEncryptedConfigFile", () => {
  it("flags the two encrypted stores and nothing else", () => {
    expect(isEncryptedConfigFile("secrets.toml.enc")).toBe(true);
    expect(isEncryptedConfigFile("agent-keys.toml.enc")).toBe(true);
    expect(isEncryptedConfigFile("config.toml")).toBe(false);
    expect(isEncryptedConfigFile("a2a-keys.toml")).toBe(false);
  });
});

describe("encryptedRestoreHint", () => {
  it("names what restoring each encrypted store does", () => {
    expect(encryptedRestoreHint("agent-keys.toml.enc")).toContain("agent keys");
    expect(encryptedRestoreHint("secrets.toml.enc")).toContain("secrets");
  });
});

describe("historyRepos", () => {
  it("shows an agent's workspace and config, or the team's and the hub's for no agent", () => {
    expect(historyRepos("atlas")).toEqual(["workspace", "agent_config"]);
    expect(historyRepos(null)).toEqual(["team", "hub"]);
  });
});

describe("undoReport", () => {
  const outcome = (reverted: string[], skipped: string[]): UndoOutcome => ({
    checkpoint_id: "c",
    reverted_paths: reverted,
    skipped_paths: skipped,
  });

  it("names what was put back and what was left alone", () => {
    expect(undoReport(outcome(["SOUL.md"], []))).toBe("Put back SOUL.md.");
    expect(undoReport(outcome(["a.md"], ["b.md", "c.md"]))).toBe(
      "Put back a.md. Left b.md, c.md alone, because they changed again since.",
    );
    expect(undoReport(outcome([], ["b.md"]))).toBe(
      "Left b.md alone, because it changed again since.",
    );
    expect(undoReport(outcome([], []))).toBe("Nothing needed undoing.");
  });
});

describe("diffLineKind", () => {
  it("tells added, removed, header and context lines apart", () => {
    const lines = ["+++ b/a", "--- a/a", "@@ -1 +1 @@", "+new", "-old", " same"];
    expect(lines.map(diffLineKind)).toEqual(["meta", "meta", "meta", "add", "remove", "context"]);
  });
});
