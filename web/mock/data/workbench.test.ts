import { runInNewContext } from "node:vm";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { fetchJson, startMockServer, type MockServerHarness } from "../test-support";
import { MOCK_WORKBENCH_ARTIFACT } from "./workbench";

type Listener = () => void;

/** The part of a page element the sample's script uses. */
interface StubElement {
  value: string;
  textContent: string;
  listeners: Map<string, Listener[]>;
  addEventListener: (type: string, listener: Listener) => void;
}

function stubElement(value = ""): StubElement {
  const listeners = new Map<string, Listener[]>();
  return {
    value,
    textContent: "",
    listeners,
    addEventListener: (type, listener) => {
      listeners.set(type, [...(listeners.get(type) ?? []), listener]);
    },
  };
}

interface SamplePage {
  click: (id: string) => void;
  each: StubElement;
  ask: Mock<(prompt: string, options: { agent: string }) => Promise<{ content: string }>>;
  startSession: Mock<(options: { agent: string; prompt: string }) => Promise<object>>;
}

/**
 * Run the sample page's own script against stub elements and a stub SDK, so
 * a click reaches the `residuum` calls the real page would make.
 */
function openSamplePage(): SamplePage {
  const script = /<script>([\s\S]*)<\/script>/.exec(MOCK_WORKBENCH_ARTIFACT)?.[1] ?? "";
  const elements = new Map([
    ["bill", stubElement("84")],
    ["people", stubElement("3")],
    ["each", stubElement()],
    ["ask", stubElement()],
    ["burst", stubElement()],
    ["spawn", stubElement()],
  ]);
  const ask = vi.fn((_prompt: string, _options: { agent: string }) =>
    Promise.resolve({ content: "fine" }),
  );
  const startSession = vi.fn((_options: { agent: string; prompt: string }) => Promise.resolve({}));
  runInNewContext(script, {
    document: {
      getElementById: (id: string) => elements.get(id),
      querySelectorAll: () => [elements.get("bill"), elements.get("people")],
    },
    residuum: { ask, sessions: { start: startSession } },
    alert: vi.fn(),
  });
  return {
    click: (id) => {
      for (const listener of elements.get(id)?.listeners.get("click") ?? []) listener();
    },
    each: elements.get("each") ?? stubElement(),
    ask,
    startSession,
  };
}

describe("the sample artifact", () => {
  it("splits the bill", () => {
    expect(openSamplePage().each.textContent).toBe("33.60 each, with 20% tip");
  });

  it("names atlas when it asks, so the SDK sends the call instead of rejecting it", () => {
    const page = openSamplePage();
    page.click("ask");
    expect(page.ask).toHaveBeenCalledTimes(1);
    expect(page.ask).toHaveBeenCalledWith(expect.stringContaining("33.60 each"), {
      agent: "atlas",
    });
  });

  it("fires three calls at once, each on atlas", () => {
    const page = openSamplePage();
    page.click("burst");
    expect(page.ask).toHaveBeenCalledTimes(3);
    expect(new Set(page.ask.mock.calls.map(([prompt]) => prompt)).size).toBe(3);
    for (const [, options] of page.ask.mock.calls) expect(options).toEqual({ agent: "atlas" });
  });

  it("starts its background session on atlas", () => {
    const page = openSamplePage();
    page.click("spawn");
    expect(page.startSession).toHaveBeenCalledWith({
      agent: "atlas",
      prompt: expect.stringContaining("tip split") as unknown as string,
    });
  });

  describe("against the mock", () => {
    let mock: MockServerHarness;

    beforeEach(async () => {
      mock = await startMockServer({ deterministic: true });
    });

    afterEach(async () => {
      await mock.close();
    });

    const asTheArtifact = (
      path: string,
      body: unknown,
    ): Promise<{ status: number; body: unknown }> =>
      fetchJson(`${mock.baseUrl}${path}`, {
        method: "POST",
        headers: { "X-Residuum-Artifact": "tip-splitter" },
        body: JSON.stringify(body),
      });

    it("reaches a running agent with its model calls and its sessions", async () => {
      expect(mock.hub.agents.get("atlas")?.runState).toBe("running");
      const page = openSamplePage();
      page.click("burst");
      const prompt = page.ask.mock.calls[0]?.[0] ?? "";
      const call = await asTheArtifact("/api/agents/atlas/model/complete", { prompt });
      expect(call.status).toBe(200);
      const session = await asTheArtifact("/api/agents/atlas/sessions", { prompt: "Check it." });
      expect(session.status).toBe(202);
      expect(session.body).toEqual({ address: expect.stringMatching(/^artifact-/) as unknown });
    });
  });
});
