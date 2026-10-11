import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { agentInboxRoutes } from "./agent-inbox";
import { createMockEnv } from "./env";
import { fetchJson, fetchText, startRouteHarness, type RouteHarness } from "./test-support";

describe("the agent inbox route", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(agentInboxRoutes, createMockEnv({ deterministic: true }));
  });

  afterEach(async () => {
    await harness.close();
  });

  const add = (
    body: unknown,
    headers: Record<string, string> = {},
  ): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}/api/agent-inbox`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });

  it("adds an item titled by the first line of its body, from the web", async () => {
    const res = await add({ body: "Check the backup job\nIt failed twice." });
    expect(res).toEqual({ status: 200, body: { id: "20260314_check_the_backup_job" } });
    expect(harness.state.agentInbox).toEqual([
      {
        id: "20260314_check_the_backup_job",
        title: "Check the backup job",
        body: "Check the backup job\nIt failed twice.",
        source: "web",
        timestamp: harness.state.env.clock.iso(),
      },
    ]);
  });

  it("takes the title it is given, and names the artifact that sent it as the source", async () => {
    await add(
      { title: "Tip report", body: "Split 3 ways." },
      { "X-Residuum-Artifact": "tip-splitter" },
    );
    expect(harness.state.agentInbox[0]).toMatchObject({
      title: "Tip report",
      source: "artifact:tip-splitter",
    });
  });

  it("numbers an item whose title the day already has", async () => {
    const ids: string[] = [];
    for (let i = 0; i < 3; i++)
      ids.push(((await add({ body: "Same title" })).body as { id: string }).id);
    expect(ids).toEqual(["20260314_same_title", "20260314_same_title_2", "20260314_same_title_3"]);
  });

  it("names the day alone for a title with no words", async () => {
    expect((await add({ body: "???" })).body).toEqual({ id: "20260314" });
  });

  it("refuses a blank body with 400, and a body that isn't there with 422", async () => {
    expect(
      await fetchText(`${harness.baseUrl}/api/agent-inbox`, postJson({ body: "  \n" })),
    ).toEqual({
      status: 400,
      body: "body must not be blank",
    });
    const missing = await fetchText(
      `${harness.baseUrl}/api/agent-inbox`,
      postJson({ title: "no body" }),
    );
    expect(missing.status).toBe(422);
  });

  it("refuses an artifact header that names no artifact", async () => {
    const res = await fetchText(`${harness.baseUrl}/api/agent-inbox`, {
      ...postJson({ body: "hello" }),
      headers: { "Content-Type": "application/json", "X-Residuum-Artifact": "Not A Name" },
    });
    expect(res.status).toBe(400);
    expect(res.body).toContain("must name an artifact");
    expect(harness.state.agentInbox).toEqual([]);
  });
});

function postJson(body: unknown): RequestInit {
  return {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  };
}
