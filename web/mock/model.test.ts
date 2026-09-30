import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createModelRoutes, modelRoutes } from "./model";
import { fetchJson, startRouteHarness, type RouteHarness } from "./test-support";

describe("model call route", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(createModelRoutes(0));
  });

  afterEach(async () => {
    await harness.close();
  });

  const complete = (
    body: unknown,
    headers: Record<string, string> = {},
  ): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}/api/model/complete`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });

  it("answers a prompt with the model's reply and the tokens it used", async () => {
    const res = await complete({ prompt: "Is 33.60 each right?" });
    expect(res).toEqual({
      status: 200,
      body: {
        content: "Mock model reply to: Is 33.60 each right?",
        model: "mock/small",
        usage: { input_tokens: 20, output_tokens: 41 },
      },
    });
  });

  it("answers a conversation about its last message, ahead of any prompt", async () => {
    const res = await complete({
      prompt: "ignored",
      system: "Be brief.",
      messages: [
        { role: "user", content: "first" },
        { role: "assistant", content: "second" },
        { role: "user", content: "third" },
      ],
    });
    expect(res.body).toMatchObject({ content: "Mock model reply to: third" });
  });

  it("answers a call that asks for a schema with parsed JSON of that shape", async () => {
    const res = await complete({
      prompt: "split it",
      schema: {
        type: "object",
        properties: {
          each: { type: "number" },
          note: { type: "string" },
          settled: { type: "boolean" },
          people: { type: "array", items: { type: "string" } },
          tip: { enum: ["low", "high"] },
          nested: { type: "object", properties: { id: { type: "integer" } } },
        },
      },
    });
    const sample = {
      each: 0,
      note: "mock",
      settled: false,
      people: [],
      tip: "low",
      nested: { id: 0 },
    };
    expect(res.status).toBe(200);
    expect(res.body).toMatchObject({ json: sample, content: JSON.stringify(sample) });
  });

  it("leaves out json when no schema was asked for", async () => {
    const res = await complete({ prompt: "hi", schema: null });
    expect(res.body).not.toHaveProperty("json");
  });

  it.each([
    [{}, 400, 'request needs a non-empty "prompt" or "messages"'],
    [{ prompt: "  " }, 400, 'request needs a non-empty "prompt" or "messages"'],
    [{ messages: [] }, 400, '"messages" must not be empty'],
    [
      { messages: [{ role: "system", content: "x" }] },
      400,
      'message role must be "user" or "assistant", got "system"',
    ],
    [{ messages: [{ role: "user", content: " " }] }, 400, "message content must not be empty"],
  ])("refuses the request %j with %i", async (body, status, error) => {
    expect(await complete(body)).toEqual({ status, body: { error } });
  });

  it("answers 422 for messages that aren't a list of role and content", async () => {
    expect((await complete({ messages: "hello" })).status).toBe(422);
    expect((await complete({ messages: [{ role: "user" }] })).status).toBe(422);
  });

  it("accepts a call from an artifact, and refuses a malformed artifact name", async () => {
    expect(
      (await complete({ prompt: "hi" }, { "X-Residuum-Artifact": "tip-splitter" })).status,
    ).toBe(200);
    expect(await complete({ prompt: "hi" }, { "X-Residuum-Artifact": "Not An Artifact" })).toEqual({
      status: 400,
      body: { error: 'the x-residuum-artifact header must name an artifact, like "wiki-graph"' },
    });
  });

  it("holds a good call for the delay, and refuses a bad one at once", async () => {
    const slow = await startRouteHarness(createModelRoutes(250));
    try {
      const url = `${slow.baseUrl}/api/model/complete`;
      const post = (body: unknown): Promise<{ status: number; body: unknown }> =>
        fetchJson(url, { method: "POST", body: JSON.stringify(body) });

      const refusedAt = Date.now();
      expect((await post({})).status).toBe(400);
      expect(Date.now() - refusedAt).toBeLessThan(200);

      const answeredAt = Date.now();
      expect((await post({ prompt: "hi" })).status).toBe(200);
      expect(Date.now() - answeredAt).toBeGreaterThanOrEqual(240);
    } finally {
      await slow.close();
    }
  });

  it("is served with the delay that shows a call in flight", () => {
    expect(modelRoutes).toEqual([
      expect.objectContaining({ method: "POST", pattern: "/api/model/complete" }),
    ]);
  });
});
