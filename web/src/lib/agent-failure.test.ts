import { afterEach, describe, expect, it, vi } from "vitest";
import { failureLine, fixSettingsSection } from "./agent-failure";
import type { Diagnostic, ValidateResponse } from "./types";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function atPath(path: string): Diagnostic {
  return { severity: "error", message: "bad value", location: { kind: "path", path } };
}

/** Answer the repair routes: each file's text, and what validating it reports. */
function serveRepair(
  validated: { providers?: ValidateResponse | "fail"; config?: ValidateResponse | "fail" },
  requests: string[] = [],
): string[] {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      requests.push(`${init?.method ?? "GET"} ${url}`);
      if (url.endsWith("/raw")) return Promise.resolve(new Response("text", { status: 200 }));
      const file = url.includes("/providers/") ? "providers" : "config";
      const answer = validated[file] ?? { valid: true };
      return Promise.resolve(answer === "fail" ? json({ error: "unreadable" }, 500) : json(answer));
    }),
  );
  return requests;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("failureLine", () => {
  it("says in plain words why each kind of failure stops an agent", () => {
    expect(failureLine("config")).toContain("settings");
    expect(failureLine("port_conflict")).toContain("port");
    expect(failureLine("crash")).toContain("unexpectedly");
    expect(failureLine(undefined)).toBe(failureLine("other"));
  });
});

describe("fixSettingsSection", () => {
  it("opens the section of the first problem a form holds, checking providers first", async () => {
    const requests = serveRepair({
      providers: { valid: false, diagnostics: [atPath("models.main")] },
      config: { valid: false, diagnostics: [atPath("memory.observer_threshold_tokens")] },
    });
    expect(await fixSettingsSection("brittle")).toBe("model");
    expect(requests).toEqual([
      "GET /api/agents/brittle/providers/raw",
      "POST /api/agents/brittle/providers/validate",
    ]);
  });

  it("goes on to config.toml when providers.toml has no problem a form holds", async () => {
    serveRepair({
      providers: { valid: false, diagnostics: [{ severity: "error", message: "no path" }] },
      config: { valid: false, diagnostics: [atPath("discord.token")] },
    });
    expect(await fixSettingsSection("brittle")).toBe("connections");
  });

  it("falls back to Raw config when no problem names a setting, or the files can't be read", async () => {
    serveRepair({ providers: { valid: true }, config: { valid: true } });
    expect(await fixSettingsSection("brittle")).toBe("raw");

    serveRepair({ providers: "fail", config: "fail" });
    expect(await fixSettingsSection("brittle")).toBe("raw");
  });
});
