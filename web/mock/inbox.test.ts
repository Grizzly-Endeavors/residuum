import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { attachmentUrl, inboxRoutes, mockAttachment } from "./inbox";
import type { MockState, UserInboxAttachment, UserInboxItem } from "./state";
import {
  startMockServer,
  startRouteHarness,
  type MockServerHarness,
  type RouteHarness,
} from "./test-support";

const REPORT: UserInboxAttachment = {
  filename: 'q3 "final".txt',
  mime_type: "text/plain",
  size: 12,
  url: "",
};
const CHART: UserInboxAttachment = {
  filename: "chart.png",
  mime_type: "image/png",
  size: 70,
  url: "",
};

/** The sample item `mock_1`, which the tests attach files to. */
function sampleItem(state: MockState): UserInboxItem {
  const item = state.inboxItems.find((candidate) => candidate.id === "mock_1");
  if (item === undefined) throw new Error("the sample inbox has no mock_1");
  return item;
}

describe("attachment urls", () => {
  it("point at the route of the inbox the item is in, under the owning agent", () => {
    expect(attachmentUrl("atlas", "user", "mock_1", 1)).toBe(
      "/api/agents/atlas/inbox/mock_1/attachments/1",
    );
    expect(attachmentUrl("atlas", "agent", "mock_1", 0)).toBe(
      "/api/agents/atlas/inbox/agent/mock_1/attachments/0",
    );
  });

  it("escape the agent and the id like the backend's path segments", () => {
    expect(attachmentUrl("at las", "user", "a/b'c", 0)).toBe(
      "/api/agents/at%20las/inbox/a%2Fb%27c/attachments/0",
    );
  });
});

describe("attachment routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(inboxRoutes);
    sampleItem(harness.state).attachments = [REPORT, CHART];
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/inbox${path}`;

  it("serves an attachment by its position, inline, under its type", async () => {
    const res = await fetch(url("/mock_1/attachments/0"));
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/plain");
    expect(res.headers.get("content-disposition")).toBe('inline; filename="q3 final.txt"');
    expect(await res.text()).toContain('q3 "final".txt');
  });

  it("serves an image attachment as a PNG", async () => {
    const res = await fetch(url("/mock_1/attachments/1"));
    expect(res.headers.get("content-type")).toBe("image/png");
    const bytes = new Uint8Array(await res.arrayBuffer());
    expect(Array.from(bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it("takes the id with or without .json", async () => {
    expect((await fetch(url("/mock_1.json/attachments/0"))).status).toBe(200);
  });

  it("keeps serving an item's attachments after it is archived", async () => {
    const item = sampleItem(harness.state);
    harness.state.inboxItems = harness.state.inboxItems.filter((candidate) => candidate !== item);
    harness.state.inboxArchive.push(item);
    const res = await fetch(url("/mock_1/attachments/0"));
    expect(res.status).toBe(200);
  });

  it("answers 404 for an item or position that isn't there, and 400 for a position that isn't a number", async () => {
    expect((await fetch(url("/ghost/attachments/0"))).status).toBe(404);
    expect((await fetch(url("/mock_1/attachments/2"))).status).toBe(404);
    expect((await fetch(url("/mock_2/attachments/0"))).status).toBe(404);
    expect((await fetch(url("/mock_1/attachments/first"))).status).toBe(400);
  });
});

describe("attachment routes over the whole mock", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer();
  });

  afterEach(async () => {
    await mock.close();
  });

  const at = (agent: string, path: string): string =>
    `${mock.baseUrl}/api/agents/${agent}/inbox${path}`;

  const stateOf = (agent: string): MockState => {
    const found = mock.hub.agents.get(agent);
    if (found === undefined) throw new Error(`no agent ${agent}`);
    return found.state;
  };

  it("keeps every agent's attachments its own", async () => {
    sampleItem(stateOf("atlas")).attachments = [mockAttachment("note.txt", "text/plain")];
    expect((await fetch(at("atlas", "/mock_1/attachments/0"))).status).toBe(200);
    expect((await fetch(at("scout", "/mock_1/attachments/0"))).status).toBe(404);
  });

  it.each(["drifter", "brittle"])(
    "answers 404 for an attachment of %s, which has never run",
    async (agent) => {
      expect((await fetch(at(agent, "/mock_1/attachments/0"))).status).toBe(404);
    },
  );

  it("serves an agent's attachments while it is stopped", async () => {
    sampleItem(stateOf("atlas")).attachments = [mockAttachment("note.txt", "text/plain")];
    await fetch(`${mock.baseUrl}/api/hub/agents/atlas/stop`, { method: "POST" });
    expect((await fetch(at("atlas", "/mock_1/attachments/0"))).status).toBe(200);
  });
});
