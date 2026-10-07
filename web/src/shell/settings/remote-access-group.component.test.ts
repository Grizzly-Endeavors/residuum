import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteAccessStatus } from "../../lib/generated/RemoteAccessStatus";
import { toast } from "../../lib/toast.svelte";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import RemoteAccessGroup from "./RemoteAccessGroup.svelte";

let status: RemoteAccessStatus;
let calls: string[];

function ready(): RemoteAccessStatus {
  return {
    state: "ready",
    detail: null,
    user: "bear",
    slug: "laptop",
    hosts: {
      ui: "bear.agent-residuum.com",
      workbench: "bear.workbench.agent-residuum.com",
      instance: "laptop.bear.agent-residuum.com",
    },
    certificate: { not_after: "2026-12-01T00:00:00Z", renews_at: "2026-11-10T00:00:00Z" },
    pins: [],
    recovery_code_pending: false,
    recovery_code: null,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  status = ready();
  calls = [];
  mockFetch((url, init) => {
    calls.push(`${init?.method ?? "GET"} ${url}`);
    if (url === "/api/hub/remote-access/status") return jsonResponse(status);
    if (url.startsWith("/api/hub/remote-access/")) return new Response(null, { status: 204 });
    return new Response("", { status: 404 });
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

describe("Remote access", () => {
  it("shows the state, the addresses and the certificate", async () => {
    render(RemoteAccessGroup);
    await settle();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.getByText("https://bear.agent-residuum.com")).toBeInTheDocument();
    expect(screen.getByText("https://laptop.bear.agent-residuum.com")).toBeInTheDocument();
    expect(screen.getByText(/Certificate valid until/)).toBeInTheDocument();
  });

  it("stays out of the way when the secure tunnel isn't in use", async () => {
    status = { ...ready(), state: "legacy", hosts: null };
    render(RemoteAccessGroup);
    await settle();
    expect(screen.queryByText("Remote access")).not.toBeInTheDocument();
  });

  it("shows the recovery code until it is saved", async () => {
    status = { ...ready(), recovery_code_pending: true, recovery_code: "ABCDEFGHIJKLMNOPQRST" };
    render(RemoteAccessGroup);
    await settle();
    expect(screen.getByText("ABCDEFGHIJKLMNOPQRST")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "I've saved it" }));
    await settle();
    expect(calls).toContain("POST /api/hub/remote-access/recovery-code/saved");
  });

  it("warns about a certificate account nobody approved", async () => {
    status = {
      ...ready(),
      pins: [
        { account_uri: "https://acme.test/acct/9", slug: "stranger", own: false, known: false },
      ],
    };
    render(RemoteAccessGroup);
    await settle();
    expect(screen.getByText("Unrecognized certificate account")).toBeInTheDocument();
    expect(screen.getByText(/The instance "stranger" can get certificates/)).toBeInTheDocument();
  });

  it("explains what needs doing and offers a retry", async () => {
    status = {
      ...ready(),
      state: "needs_join",
      detail: "Another instance of yours is already set up for remote access (desktop).",
      certificate: null,
    };
    render(RemoteAccessGroup);
    await settle();
    expect(screen.getByText(/already set up for remote access/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Try again now" }));
    await settle();
    expect(calls).toContain("POST /api/hub/remote-access/retry");
  });

  it("resets the pinned accounts with a recovery code", async () => {
    render(RemoteAccessGroup);
    await settle();
    await fireEvent.click(screen.getByRole("button", { name: "Use a recovery code" }));
    await settle();
    const input = screen.getByLabelText("Recovery code");
    await fireEvent.input(input, { target: { value: "ABCDEFGHIJKLMNOPQRST" } });
    await fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await settle();
    expect(calls).toContain("POST /api/hub/remote-access/reset-pins");
  });
});
