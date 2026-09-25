import { describe, expect, it } from "vitest";
import {
  encryptedRestoreHint,
  formatBytes,
  isEncryptedConfigFile,
  triggerLabel,
} from "./checkpoints";

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
