import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, fetchHubStatus } from "./api";
import { jsonResponse, mockFetch } from "../test/component";

// An unpaired browser's API call gets a 401 carrying `device_required` from
// the gateway; the page moves to the pairing page, and the call still fails.

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubLocation(pathname: string): ReturnType<typeof vi.fn> {
  const assign = vi.fn();
  vi.stubGlobal("location", { pathname, assign });
  return assign;
}

describe("a 401 for an unpaired browser", () => {
  it("sends the page to the pairing page and still fails the call", async () => {
    const assign = stubLocation("/agent/scout");
    mockFetch(() =>
      jsonResponse({ error: "This browser isn't paired.", code: "device_required" }, 401),
    );
    await expect(fetchHubStatus()).rejects.toBeInstanceOf(ApiError);
    expect(assign).toHaveBeenCalledWith("/pair");
  });

  it("does nothing for a 401 that is not about pairing", async () => {
    const assign = stubLocation("/agent/scout");
    mockFetch(() => jsonResponse({ error: "no" }, 401));
    await expect(fetchHubStatus()).rejects.toBeInstanceOf(ApiError);
    expect(assign).not.toHaveBeenCalled();
  });
});
