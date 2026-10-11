import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceListResponse } from "../../lib/generated/DeviceListResponse";
import { toast } from "../../lib/toast.svelte";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
} from "../../test/component";
import DevicesGroup from "./DevicesGroup.svelte";

let listing: DeviceListResponse;
let calls: string[];
let linkAnswer: () => Response;

function empty(): DeviceListResponse {
  return {
    devices: [],
    pending: [],
    recovery_codes_remaining: 0,
    ui_origin: "https://bear.agent-residuum.com",
    remote: false,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  listing = empty();
  calls = [];
  linkAnswer = () =>
    jsonResponse({
      link: "https://bear.agent-residuum.com/pair#token=abc",
      qr_svg: "<svg></svg>",
      expires_in_secs: 600,
      recovery_codes: ["AAAA-BBBB-CCCC-DDDD", "EEEE-FFFF-GGGG-HHHH"],
    });
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    calls.push(`${method} ${url}`);
    if (url === "/api/hub/devices") return jsonResponse(listing);
    if (url === "/api/hub/remote-access/pair-link") return linkAnswer();
    if (url.startsWith("/api/hub/devices/")) return new Response(null, { status: 204 });
    return new Response("", { status: 404 });
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

describe("Paired browsers", () => {
  it("offers to enable remote access when nothing is paired, and shows the link, QR code and recovery codes once", async () => {
    render(DevicesGroup);
    await settle();
    expect(screen.getByText(/No browsers are paired/)).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Enable remote access" }));
    await settle();
    expect(screen.getByText("https://bear.agent-residuum.com/pair#token=abc")).toBeInTheDocument();
    expect(screen.getByAltText("QR code for the pairing link")).toBeInTheDocument();
    expect(screen.getByText("AAAA-BBBB-CCCC-DDDD")).toBeInTheDocument();
    expect(screen.getByText(/works once and expires in 10 minutes/)).toBeInTheDocument();
  });

  it("explains why a link couldn't be made", async () => {
    linkAnswer = () =>
      jsonResponse({ error: "Residuum Cloud hasn't told this install its web address yet." }, 409);
    render(DevicesGroup);
    await settle();
    await fireEvent.click(screen.getByRole("button", { name: "Enable remote access" }));
    await settle();
    const messages = [...toast.toasts.values()].map((t) => t.message).join(" ");
    expect(messages).toContain("hasn't told this install its web address");
  });

  it("lists paired browsers and revokes one", async () => {
    listing = {
      ...empty(),
      devices: [
        {
          id: "d1",
          name: "Firefox on Linux",
          created_at: "2026-10-01T00:00:00Z",
          last_seen: "2026-10-06T00:00:00Z",
          current: true,
        },
      ],
      recovery_codes_remaining: 9,
    };
    render(DevicesGroup);
    await settle();
    expect(screen.getByText("Firefox on Linux")).toBeInTheDocument();
    expect(screen.getByText("This browser")).toBeInTheDocument();
    expect(screen.getByText(/9 unused recovery codes/)).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Revoke Firefox on Linux" }));
    await settle();
    expect(calls).toContain("DELETE /api/hub/devices/d1");
  });

  it("shows a waiting request with its code, and approves it", async () => {
    render(DevicesGroup);
    await settle();
    listing = {
      ...empty(),
      pending: [
        {
          id: "p1",
          code: "K7QX2M",
          device_name: "Phone",
          created_at: "2026-10-07T00:00:00Z",
          expires_at: "2026-10-07T00:10:00Z",
        },
      ],
    };
    await advance(4000);
    expect(screen.getByText("K7QX2M")).toBeInTheDocument();
    expect(screen.getByText(/Approve only if the code matches/)).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await settle();
    expect(calls).toContain("POST /api/hub/devices/pending/p1/approve");
  });

  it("does not offer a pairing link to a browser that is itself remote", async () => {
    listing = { ...empty(), remote: true };
    render(DevicesGroup);
    await settle();
    expect(screen.queryByRole("button", { name: "Enable remote access" })).toBeNull();
  });
});
