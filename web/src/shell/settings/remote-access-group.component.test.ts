import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteAccessStatus } from "../../lib/generated/RemoteAccessStatus";
import { toast } from "../../lib/toast.svelte";
import { ConfirmHost } from "../../lib/ui";
import userEvent from "@testing-library/user-event";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import RemoteAccessGroup from "./RemoteAccessGroup.svelte";

let status: RemoteAccessStatus;
let calls: string[];
let bodies: unknown[];

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
    instances: [],
    siblings: [],
    join: null,
    pending_joins: [],
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  status = ready();
  calls = [];
  bodies = [];
  mockFetch((url, init) => {
    calls.push(`${init?.method ?? "GET"} ${url}`);
    if (typeof init?.body === "string") bodies.push(JSON.parse(init.body));
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

  it("stays out of the way when Residuum Cloud isn't set up", async () => {
    status = { ...ready(), state: "disabled", hosts: null };
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
        {
          account_uri: "https://acme.test/acct/9",
          slug: "stranger",
          own: false,
          known: false,
          removable: false,
        },
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

  describe("joining another instance", () => {
    const withPins = (): RemoteAccessStatus => ({
      ...ready(),
      pins: [
        { account_uri: "u/1", slug: "laptop", own: true, known: true, removable: false },
        { account_uri: "u/2", slug: "desktop", own: false, known: true, removable: false },
      ],
    });

    it("submits the slug that was typed", async () => {
      status = withPins();
      render(RemoteAccessGroup);
      await settle();
      await fireEvent.click(screen.getByRole("button", { name: "Join another instance" }));
      const input = screen.getByLabelText("Instance to join");
      await fireEvent.input(input, { target: { value: "desktop" } });
      await fireEvent.click(screen.getByRole("button", { name: "Join" }));
      await settle();
      expect(calls).toContain("POST /api/hub/remote-access/join");
      expect(bodies).toContainEqual({ instance: "desktop" });
    });

    it("offers the other pinned instances as choices", async () => {
      status = withPins();
      const { container } = render(RemoteAccessGroup);
      await settle();
      const options = [...container.querySelectorAll("datalist option")].map((option) =>
        option.getAttribute("value"),
      );
      expect(options).toEqual(["desktop"]);
    });

    it("won't send a name that isn't an instance slug", async () => {
      status = withPins();
      render(RemoteAccessGroup);
      await settle();
      await fireEvent.click(screen.getByRole("button", { name: "Join another instance" }));
      await fireEvent.input(screen.getByLabelText("Instance to join"), {
        target: { value: "a/../b" },
      });
      expect(screen.getByRole("button", { name: "Join" })).toBeDisabled();
      expect(screen.getByText(/1 to 24 lowercase letters/)).toBeInTheDocument();
    });

    it("is open and prominent while this instance needs another", async () => {
      status = { ...withPins(), state: "needs_join", certificate: null };
      render(RemoteAccessGroup);
      await settle();
      expect(screen.getByRole("region", { name: "Join another instance" })).toBeInTheDocument();
      expect(screen.getByLabelText("Instance to join")).toBeVisible();
    });

    it("shows the code to compare while it waits", async () => {
      status = {
        ...withPins(),
        join: { instance: "desktop", state: "waiting", code: "482916", detail: null },
      };
      render(RemoteAccessGroup);
      await settle();
      await fireEvent.click(screen.getByRole("button", { name: "Join another instance" }));
      expect(screen.getByText("482916")).toBeInTheDocument();
      expect(screen.getByText(/shows the same code/)).toBeInTheDocument();
      expect(screen.queryByLabelText("Instance to join")).not.toBeInTheDocument();
    });

    it.each([
      ["approved", "Desktop approved this instance."],
      ["denied", "Desktop said no."],
      ["failed", "Desktop didn't answer."],
    ] as const)("reports a join that %s", async (state, detail) => {
      status = { ...withPins(), join: { instance: "desktop", state, code: null, detail } };
      render(RemoteAccessGroup);
      await settle();
      await fireEvent.click(screen.getByRole("button", { name: "Join another instance" }));
      expect(screen.getByText(detail)).toBeInTheDocument();
    });
  });

  describe("approving other instances", () => {
    const pending = {
      id: "j1",
      code: "731504",
      slug: "tablet",
      display_name: "Tablet",
      in_relay_list: null,
      expires_at: "2026-12-01T00:10:00Z",
    };

    it("shows the code, the claimed names and a hint when the relay lists it", async () => {
      status = { ...ready(), pending_joins: [{ ...pending, in_relay_list: true }] };
      render(RemoteAccessGroup);
      await settle();
      expect(screen.getByText("731504")).toBeInTheDocument();
      expect(screen.getByText("tablet")).toBeInTheDocument();
      expect(screen.getByText("Tablet")).toBeInTheDocument();
      expect(screen.getByText("Residuum Cloud lists this instance.")).toBeInTheDocument();
      expect(screen.getByText(/Approve only if the code matches/)).toBeInTheDocument();
    });

    it("says when the relay doesn't list the name, and nothing when it can't tell", async () => {
      status = { ...ready(), pending_joins: [{ ...pending, in_relay_list: false }] };
      const view = render(RemoteAccessGroup);
      await settle();
      expect(
        screen.getByText("Residuum Cloud does not list an instance with this name."),
      ).toBeInTheDocument();
      view.unmount();

      status = { ...ready(), pending_joins: [pending] };
      render(RemoteAccessGroup);
      await settle();
      expect(screen.queryByText(/Residuum Cloud lists/)).not.toBeInTheDocument();
      expect(screen.queryByText(/does not list/)).not.toBeInTheDocument();
    });

    it("approves and denies by the request's id", async () => {
      status = { ...ready(), pending_joins: [pending] };
      render(RemoteAccessGroup);
      await settle();
      await fireEvent.click(screen.getByRole("button", { name: "Approve" }));
      await settle();
      expect(calls).toContain("POST /api/hub/remote-access/joins/j1/approve");
      await fireEvent.click(screen.getByRole("button", { name: "Deny" }));
      await settle();
      expect(calls).toContain("POST /api/hub/remote-access/joins/j1/deny");
    });

    it("draws a hostile name as text", async () => {
      status = {
        ...ready(),
        pending_joins: [
          {
            ...pending,
            slug: '"><script>alert(1)</script>',
            display_name: "<img src=x onerror=alert(1)>",
          },
        ],
      };
      const { container } = render(RemoteAccessGroup);
      await settle();
      expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
      expect(screen.getByText('"><script>alert(1)</script>')).toBeInTheDocument();
      expect(container.querySelector("img")).toBeNull();
      expect(container.querySelector("script")).toBeNull();
    });

    it("lists the instances that joined, as text", async () => {
      status = {
        ...ready(),
        siblings: [
          { slug: "desktop", display_name: "<b>Desktop</b>" },
          { slug: "a/../b", display_name: "Bad slug" },
        ],
      };
      const { container } = render(RemoteAccessGroup);
      await settle();
      expect(screen.getByText("<b>Desktop</b>")).toBeInTheDocument();
      expect(container.querySelector("b")).toBeNull();
      expect(screen.queryByText("Bad slug")).not.toBeInTheDocument();
    });
  });

  describe("removing certificate accounts", () => {
    const pins = (): RemoteAccessStatus["pins"] => [
      { account_uri: "u/own", slug: "laptop", own: true, known: true, removable: false },
      { account_uri: "u/known", slug: "desktop", own: false, known: true, removable: false },
      { account_uri: "u/gone", slug: "old-box", own: false, known: true, removable: true },
    ];

    it("offers Remove only for a removable account", async () => {
      status = { ...ready(), pins: pins() };
      render(RemoteAccessGroup);
      await settle();
      expect(screen.getAllByRole("button", { name: /^Remove / })).toHaveLength(1);
      expect(screen.getByRole("button", { name: "Remove old-box" })).toBeInTheDocument();
    });

    it("asks first, and removes the account once confirmed", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      status = { ...ready(), pins: pins() };
      render(RemoteAccessGroup);
      render(ConfirmHost);
      await settle();
      await user.click(screen.getByRole("button", { name: "Remove old-box" }));
      await settle();
      expect(
        await screen.findByRole("alertdialog", { name: "Remove old-box?" }),
      ).toBeInTheDocument();
      expect(screen.getByText(/no longer exists in Residuum Cloud/)).toBeInTheDocument();
      expect(screen.getByText(/stay valid until they expire/)).toBeInTheDocument();
      expect(calls).not.toContain("POST /api/hub/remote-access/pins/remove");
      await user.click(screen.getByRole("button", { name: "Remove account" }));
      await settle();
      expect(calls).toContain("POST /api/hub/remote-access/pins/remove");
      expect(bodies).toContainEqual({ account_uri: "u/gone" });
    });

    it("sends nothing when the question is cancelled", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      status = { ...ready(), pins: pins() };
      render(RemoteAccessGroup);
      render(ConfirmHost);
      await settle();
      await user.click(screen.getByRole("button", { name: "Remove old-box" }));
      await settle();
      await user.click(screen.getByRole("button", { name: "Cancel" }));
      await settle();
      expect(calls).not.toContain("POST /api/hub/remote-access/pins/remove");
    });
  });
});
