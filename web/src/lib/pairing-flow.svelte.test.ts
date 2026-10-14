import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, mockFetch } from "../test/component";
import { POLL_INTERVAL_MS, PairingFlow, type PairingEnvironment } from "./pairing-flow.svelte";

let opened: number;
let cleared: number;
let hash: string;
/** How the gateway answers each route. */
let answers: Record<string, () => Response>;
let requests: { url: string; body: unknown }[];

function environment(): PairingEnvironment {
  return {
    hash: () => hash,
    clearHash: () => {
      cleared += 1;
    },
    userAgent: () => "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
    openApp: () => {
      opened += 1;
    },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  opened = 0;
  cleared = 0;
  hash = "";
  requests = [];
  answers = {
    "/api/hub/pairing/state": () => jsonResponse({ remote: true, paired: false }),
    "/api/hub/pairing/redeem": () => jsonResponse({ device_name: "Firefox on Linux" }),
    "/api/hub/pairing/recovery": () => jsonResponse({ device_name: "Firefox on Linux" }),
    "/api/hub/pairing/requests": () =>
      jsonResponse({ request_id: "secret", code: "K7QX2M", expires_in_secs: 600 }),
    "/api/hub/pairing/requests/poll": () => jsonResponse({ status: "pending" }),
  };
  mockFetch((url, init) => {
    const body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    requests.push({ url, body });
    const answer = answers[url];
    return answer === undefined ? new Response("", { status: 404 }) : answer();
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("PairingFlow", () => {
  it("offers to ask for approval when a browser arrives without a link", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    expect(flow.phase).toBe("choose");
    expect(flow.deviceName).toBe("Firefox on Linux");
  });

  it("leaves for the app at once when this browser needs no pairing", async () => {
    answers["/api/hub/pairing/state"] = () => jsonResponse({ remote: true, paired: true });
    const flow = new PairingFlow(environment());
    await flow.start();
    expect(opened).toBe(1);
    expect(flow.phase).toBe("done");
  });

  it("needs no pairing on the machine Residuum runs on", async () => {
    answers["/api/hub/pairing/state"] = () => jsonResponse({ remote: false, paired: true });
    const flow = new PairingFlow(environment());
    await flow.start();
    expect(opened).toBe(1);
  });

  it("takes the token from a pairing link and takes it out of the address bar", async () => {
    hash = "#token=link-token";
    const flow = new PairingFlow(environment());
    await flow.start();
    expect(flow.phase).toBe("link");
    expect(cleared).toBe(1);
    flow.deviceName = "Work laptop";
    await flow.pairWithLink();
    expect(requests.at(-1)).toEqual({
      url: "/api/hub/pairing/redeem",
      body: { token: "link-token", device_name: "Work laptop" },
    });
    expect(opened).toBe(1);
  });

  it("says why a spent link didn't pair", async () => {
    hash = "#token=used";
    answers["/api/hub/pairing/redeem"] = () =>
      jsonResponse({ error: "This pairing link has expired or was already used." }, 400);
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.pairWithLink();
    expect(flow.problem).toContain("expired or was already used");
    expect(opened).toBe(0);
    expect(flow.phase).toBe("link");
  });

  it("shows the code and waits, then leaves for the app once approved", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.requestApproval();
    expect(flow.phase).toBe("waiting");
    expect(flow.code).toBe("K7QX2M");

    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(flow.phase).toBe("waiting");
    expect(requests.at(-1)).toEqual({
      url: "/api/hub/pairing/requests/poll",
      body: { request_id: "secret" },
    });

    answers["/api/hub/pairing/requests/poll"] = () => jsonResponse({ status: "approved" });
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(opened).toBe(1);

    const polls = requests.length;
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 3);
    expect(requests).toHaveLength(polls);
  });

  it("goes back to choosing, with the reason, when the request is refused or expires", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.requestApproval();
    answers["/api/hub/pairing/requests/poll"] = () => jsonResponse({ status: "denied" });
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(flow.phase).toBe("choose");
    expect(flow.problem).toContain("refused");

    await flow.requestApproval();
    answers["/api/hub/pairing/requests/poll"] = () => jsonResponse({ status: "expired" });
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(flow.problem).toContain("expired");
  });

  it("stops polling when the request is cancelled", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.requestApproval();
    flow.cancelRequest();
    const before = requests.length;
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS * 3);
    expect(requests).toHaveLength(before);
    expect(flow.phase).toBe("choose");
  });

  it("keeps waiting through a failed check", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.requestApproval();
    answers["/api/hub/pairing/requests/poll"] = () => new Response("", { status: 502 });
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(flow.phase).toBe("waiting");
    expect(flow.problem).not.toBe("");
    answers["/api/hub/pairing/requests/poll"] = () => jsonResponse({ status: "approved" });
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL_MS);
    expect(opened).toBe(1);
  });

  it("pairs with a recovery code", async () => {
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.pairWithRecoveryCode("abcd-efgh-ijkl-mnop");
    expect(requests.at(-1)).toEqual({
      url: "/api/hub/pairing/recovery",
      body: { code: "abcd-efgh-ijkl-mnop", device_name: "Firefox on Linux" },
    });
    expect(opened).toBe(1);
  });

  it("explains a rate limit instead of failing silently", async () => {
    answers["/api/hub/pairing/requests"] = () =>
      jsonResponse({ error: "Too many pairing attempts from this address." }, 429);
    const flow = new PairingFlow(environment());
    await flow.start();
    await flow.requestApproval();
    expect(flow.problem).toContain("Too many pairing attempts");
    expect(flow.phase).toBe("choose");
  });
});
