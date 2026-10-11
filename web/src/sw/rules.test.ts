import { describe, expect, it } from "vitest";
import {
  handlingOf,
  isGatewayFailure,
  parseGenerations,
  recordActivation,
  shellCacheName,
  staleCaches,
  type Handling,
} from "./rules";

const ORIGIN = "https://bear.example.com";
const PRECACHED: ReadonlySet<string> = new Set([
  "/index.html",
  "/favicon.svg",
  "/icons/icon-192.png",
  "/assets/index-abc123.js",
]);

function handling(
  path: string,
  init: { method?: string; mode?: string; origin?: string } = {},
): Handling {
  return handlingOf(
    {
      method: init.method ?? "GET",
      url: `${init.origin ?? ORIGIN}${path}`,
      mode: init.mode ?? "cors",
    },
    ORIGIN,
    PRECACHED,
  );
}

describe("what the worker answers", () => {
  it("serves the app's files from the cache first", () => {
    expect(handling("/assets/index-abc123.js")).toBe("cache-first");
    // A chunk only a page built earlier asks for is not in this build's list.
    expect(handling("/assets/Settings-old999.js")).toBe("cache-first");
    expect(handling("/icons/icon-192.png")).toBe("cache-first");
    expect(handling("/favicon.svg")).toBe("cache-first");
  });

  it("loads a client route from the network first", () => {
    for (const path of ["/", "/home", "/agent/atlas/files", "/settings/memory", "/index.html"]) {
      expect(handling(path, { mode: "navigate" }), path).toBe("navigate");
    }
  });

  it("never touches the hub's API, sockets, webhooks or the cloud callback", () => {
    for (const path of [
      "/api/hub/agents",
      "/api/agents/atlas/ws",
      "/api",
      "/ws/extra",
      "/webhook/github",
      "/cloud/callback",
    ]) {
      expect(handling(path), path).toBe("pass");
      expect(handling(path, { mode: "navigate" }), `${path} as a page load`).toBe("pass");
    }
  });

  it("is not fooled by a first segment that only starts like the hub's", () => {
    expect(handling("/apiary", { mode: "navigate" })).toBe("navigate");
    expect(handling("/cloudy/day", { mode: "navigate" })).toBe("navigate");
  });

  it("leaves a hub API path alone even when the list names it", () => {
    const listed = new Set(["/api/hub/agents"]);
    expect(
      handlingOf({ method: "GET", url: `${ORIGIN}/api/hub/agents`, mode: "cors" }, ORIGIN, listed),
    ).toBe("pass");
  });

  it("leaves requests that change something to the browser", () => {
    for (const method of ["POST", "PUT", "PATCH", "DELETE", "HEAD"]) {
      expect(handling("/assets/index-abc123.js", { method }), method).toBe("pass");
      expect(handling("/home", { method, mode: "navigate" }), method).toBe("pass");
    }
  });

  it("leaves other origins alone, the artifacts origin included", () => {
    expect(
      handling("/assets/index-abc123.js", { origin: "https://bear.workbench.example.com" }),
    ).toBe("pass");
    expect(handling("/home", { origin: "http://localhost:5180", mode: "navigate" })).toBe("pass");
  });

  it("leaves a file the app doesn't hold to the network", () => {
    expect(handling("/mcp-catalog.json")).toBe("pass");
    expect(handling("/manifest.webmanifest")).toBe("pass");
    expect(handling("/mcp-catalog.json", { mode: "navigate" })).toBe("pass");
  });
});

describe("a page load that finds the hub down", () => {
  it("counts the gateway statuses a relay or reverse proxy answers with", () => {
    expect([502, 503, 504].every(isGatewayFailure)).toBe(true);
  });

  it("leaves every other answer as the page's own", () => {
    for (const status of [200, 204, 301, 302, 304, 401, 403, 404, 500]) {
      expect(isGatewayFailure(status), String(status)).toBe(false);
    }
  });
});

describe("which shell versions are kept", () => {
  it("records the first version with nothing before it", () => {
    expect(recordActivation(null, "aaa")).toEqual({ current: "aaa", previous: null });
  });

  it("keeps the version a new one replaces", () => {
    const first = recordActivation(null, "aaa");
    expect(recordActivation(first, "bbb")).toEqual({ current: "bbb", previous: "aaa" });
  });

  it("forgets a version once two newer ones have been active", () => {
    let recorded = recordActivation(null, "aaa");
    recorded = recordActivation(recorded, "bbb");
    recorded = recordActivation(recorded, "ccc");
    expect(recorded).toEqual({ current: "ccc", previous: "bbb" });
  });

  it("changes nothing when the active version activates again", () => {
    const recorded = { current: "bbb", previous: "aaa" };
    expect(recordActivation(recorded, "bbb")).toEqual(recorded);
  });

  it("deletes every shell cache but the two kept, and no other cache", () => {
    const existing = [
      shellCacheName("aaa"),
      shellCacheName("bbb"),
      shellCacheName("ccc"),
      shellCacheName("ddd"),
      "residuum-generations",
      "someone-elses-cache",
    ];
    expect(staleCaches(existing, { current: "ddd", previous: "ccc" })).toEqual([
      shellCacheName("aaa"),
      shellCacheName("bbb"),
    ]);
    expect(staleCaches(existing, { current: "bbb", previous: null })).toEqual([
      shellCacheName("aaa"),
      shellCacheName("ccc"),
      shellCacheName("ddd"),
    ]);
  });

  it("reads back what it recorded, and nothing else", () => {
    expect(parseGenerations({ current: "bbb", previous: "aaa" })).toEqual({
      current: "bbb",
      previous: "aaa",
    });
    expect(parseGenerations({ current: "bbb", previous: null })).toEqual({
      current: "bbb",
      previous: null,
    });
    for (const unreadable of [
      null,
      "bbb",
      3,
      {},
      { current: 3, previous: null },
      { current: "b" },
    ]) {
      expect(parseGenerations(unreadable), JSON.stringify(unreadable)).toBeNull();
    }
  });
});
