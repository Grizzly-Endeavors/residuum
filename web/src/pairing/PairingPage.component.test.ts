import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { POLL_INTERVAL_MS, PairingFlow } from "../lib/pairing-flow.svelte";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
} from "../test/component";
import PairingPage from "./PairingPage.svelte";

let opened = 0;
let hash = "";
let pollStatus = "pending";

function flow(): PairingFlow {
  return new PairingFlow({
    hash: () => hash,
    clearHash: () => {},
    userAgent: () => "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
    openApp: () => {
      opened += 1;
    },
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  opened = 0;
  hash = "";
  pollStatus = "pending";
  mockFetch((url) => {
    switch (url) {
      case "/api/hub/pairing/state":
        return jsonResponse({ remote: true, paired: false });
      case "/api/hub/pairing/requests":
        return jsonResponse({ request_id: "s", code: "K7QX2M", expires_in_secs: 600 });
      case "/api/hub/pairing/requests/poll":
        return jsonResponse({ status: pollStatus });
      case "/api/hub/pairing/redeem":
        return jsonResponse({ device_name: "Firefox on Linux" });
      default:
        return new Response("", { status: 404 });
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the pairing page", () => {
  it("asks for approval, shows the code to match, and opens the app once approved", async () => {
    render(PairingPage, { flow: flow() });
    await settle();
    expect(screen.getByRole("heading", { name: "Pair this browser" })).toBeInTheDocument();
    expect(screen.getByLabelText("Name for this browser")).toHaveValue("Firefox on Linux");

    await fireEvent.click(screen.getByRole("button", { name: "Ask for approval" }));
    await settle();
    expect(screen.getByText("K7QX2M")).toBeInTheDocument();
    expect(screen.getByText(/Approve only if the codes match/)).toBeInTheDocument();

    pollStatus = "approved";
    await advance(POLL_INTERVAL_MS);
    expect(opened).toBe(1);
  });

  it("pairs in one step from a pairing link", async () => {
    hash = "#token=abc";
    render(PairingPage, { flow: flow() });
    await settle();
    expect(screen.getByText(/You opened a pairing link/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Pair this browser" }));
    await settle();
    expect(opened).toBe(1);
  });

  it("keeps the recovery code behind a disclosure and disables it until one is typed", async () => {
    render(PairingPage, { flow: flow() });
    await settle();
    await fireEvent.click(screen.getByText("Use a recovery code instead"));
    const submit = screen.getByRole("button", { name: "Pair with recovery code" });
    expect(submit).toBeDisabled();
    await fireEvent.input(screen.getByLabelText("Recovery code"), {
      target: { value: "ABCD-EFGH-IJKL-MNOP" },
    });
    expect(submit).toBeEnabled();
  });
});
