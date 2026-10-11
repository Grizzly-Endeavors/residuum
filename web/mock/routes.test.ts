import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { json } from "./http";
import { decodedParam, type Route } from "./routes";
import { fetchJson, startRouteHarness, type RouteHarness } from "./test-support";

const routes: Route[] = [
  {
    method: "GET",
    pattern: "/api/exact",
    handler: ({ res }) => {
      json(res, 200, { which: "get" });
    },
  },
  {
    method: "POST",
    pattern: "/api/exact",
    handler: ({ res }) => {
      json(res, 200, { which: "post" });
    },
  },
  {
    method: "GET",
    pattern: /^\/api\/items\/([^/]+)\/parts\/([^/]+)$/,
    handler: (ctx) => {
      json(ctx.res, 200, {
        raw: [...ctx.params],
        decoded: [decodedParam(ctx, 0), decodedParam(ctx, 1)],
      });
    },
  },
  {
    method: "GET",
    pattern: /^\/api\/items\/(.+)$/,
    handler: ({ res }) => {
      json(res, 200, { which: "greedy" });
    },
  },
  {
    method: "GET",
    pattern: "/api/boom",
    handler: () => {
      throw new Error("handler failed");
    },
  },
  {
    method: "GET",
    pattern: "/api/slow",
    handler: async ({ res, query }) => {
      await Promise.resolve();
      json(res, 200, { waited: query.get("for") });
    },
  },
];

describe("dispatchRoute", () => {
  let harness: RouteHarness;

  beforeAll(async () => {
    harness = await startRouteHarness(routes);
  });

  afterAll(async () => {
    await harness.close();
  });

  it("matches an exact path by method", async () => {
    expect(await fetchJson(`${harness.baseUrl}/api/exact`)).toEqual({
      status: 200,
      body: { which: "get" },
    });
    expect(await fetchJson(`${harness.baseUrl}/api/exact`, { method: "POST" })).toEqual({
      status: 200,
      body: { which: "post" },
    });
  });

  it("answers nothing for a method or path no route has", async () => {
    const wrongMethod = await fetchJson(`${harness.baseUrl}/api/exact`, { method: "DELETE" });
    expect(wrongMethod.status).toBe(404);
    const unknownPath = await fetchJson(`${harness.baseUrl}/api/missing`);
    expect(unknownPath.status).toBe(404);
  });

  it("runs the first matching route in table order", async () => {
    const specific = await fetchJson(`${harness.baseUrl}/api/items/a/parts/b`);
    expect(specific.body).toMatchObject({ raw: ["a", "b"] });
    const greedy = await fetchJson(`${harness.baseUrl}/api/items/a/other`);
    expect(greedy.body).toEqual({ which: "greedy" });
  });

  it("hands over capture groups still encoded, and decodes them on request", async () => {
    const { body } = await fetchJson(`${harness.baseUrl}/api/items/a%20b/parts/c%2Fd`);
    expect(body).toEqual({ raw: ["a%20b", "c%2Fd"], decoded: ["a b", "c/d"] });
  });

  it("awaits an async handler and passes the query", async () => {
    const { body } = await fetchJson(`${harness.baseUrl}/api/slow?for=a-moment`);
    expect(body).toEqual({ waited: "a-moment" });
  });

  it("lets a throwing handler surface to the caller", async () => {
    const { status, body } = await fetchJson(`${harness.baseUrl}/api/boom`);
    expect(status).toBe(500);
    expect(body).toEqual({ error: "handler failed" });
  });
});
