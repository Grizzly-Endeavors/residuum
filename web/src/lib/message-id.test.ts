import { afterEach, describe, expect, it, vi } from "vitest";
import { newMessageId } from "./message-id";

const FORMAT = /^web-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("newMessageId", () => {
  it("never repeats, so a reload can't reuse the id of an earlier turn", () => {
    const ids = new Set(Array.from({ length: 2_000 }, () => newMessageId()));
    expect(ids.size).toBe(2_000);
  });

  it("names the web client and carries a UUID", () => {
    expect(newMessageId()).toMatch(FORMAT);
  });

  it("makes one where crypto.randomUUID is missing, as in a page served over plain HTTP", () => {
    const real = globalThis.crypto;
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array) => real.getRandomValues(bytes),
    });
    const ids = new Set(Array.from({ length: 200 }, () => newMessageId()));
    expect(ids.size).toBe(200);
    for (const id of ids) expect(id).toMatch(FORMAT);
  });

  it("makes one where there is no crypto at all", () => {
    vi.stubGlobal("crypto", undefined);
    const ids = new Set(Array.from({ length: 200 }, () => newMessageId()));
    expect(ids.size).toBe(200);
    for (const id of ids) expect(id).toMatch(FORMAT);
  });
});
