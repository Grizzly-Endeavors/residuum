import { afterEach, describe, expect, it, vi } from "vitest";
import type * as CacheModule from "./cache";

/** A working `localStorage` holding `entries`, which the Node test environment lacks. */
function stubStorage(entries: Record<string, string>): Map<string, string> {
  const data = new Map(Object.entries(entries));
  vi.stubGlobal("localStorage", {
    get length() {
      return data.size;
    },
    key: (index: number) => [...data.keys()][index] ?? null,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
    removeItem: (key: string) => {
      data.delete(key);
    },
    clear: () => {
      data.clear();
    },
  });
  return data;
}

/** A fresh copy of the cache module, as a new page load would get. */
async function loadCache(): Promise<typeof CacheModule> {
  vi.resetModules();
  return import("./cache");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("fetch cache", () => {
  it("never answers from what an earlier page load stored", async () => {
    stubStorage({
      "residuum:cache:v1:GET /api/agents/mist/chat/history?episode=ep-001": '"another instance"',
    });
    const { cachedFetch } = await loadCache();

    const value = await cachedFetch("GET /api/agents/mist/chat/history?episode=ep-001", () =>
      Promise.resolve("this instance"),
    );

    expect(value).toBe("this instance");
  });

  it("clears entries an earlier page load left in storage and keeps everything else", async () => {
    const data = stubStorage({
      "residuum:cache:v1:GET /api/agents/mist/config/raw": '"timezone = \\"UTC\\""',
      "residuum:agent": "mist",
    });

    await loadCache();

    expect([...data.keys()]).toEqual(["residuum:agent"]);
  });

  it("keeps what it fetches out of storage", async () => {
    const data = stubStorage({});
    const { cachedFetch } = await loadCache();

    await cachedFetch("GET /api/timezone", () => Promise.resolve("UTC"));

    expect(data.size).toBe(0);
  });

  it("answers a repeat read from memory until the entry is invalidated", async () => {
    stubStorage({});
    const { cachedFetch, invalidate } = await loadCache();
    const fetchValue = vi.fn(() => Promise.resolve("value"));

    await cachedFetch("GET /api/timezone", fetchValue);
    await cachedFetch("GET /api/timezone", fetchValue);
    expect(fetchValue).toHaveBeenCalledTimes(1);

    invalidate("GET /api/timezone");
    await cachedFetch("GET /api/timezone", fetchValue);
    expect(fetchValue).toHaveBeenCalledTimes(2);
  });

  it("does not keep a failed fetch", async () => {
    stubStorage({});
    const { cachedFetch } = await loadCache();

    await expect(
      cachedFetch("GET /api/timezone", () => Promise.reject(new Error("offline"))),
    ).rejects.toThrow("offline");

    await expect(cachedFetch("GET /api/timezone", () => Promise.resolve("UTC"))).resolves.toBe(
      "UTC",
    );
  });
});
