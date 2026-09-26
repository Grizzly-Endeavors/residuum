import type { RollbackNoticeResponse, UpdateStatusResponse } from "../../lib/types";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
} from "../../test/component";
import Update from "./Update.svelte";

// Same windows Update.svelte polls with. The test drives them with a fake
// clock instead of waiting out a real restart.
const RESTART_POLL_MS = 1500;
const RESTART_TIMEOUT_MS = 90_000;

function updateStatus(overrides: Partial<UpdateStatusResponse> = {}): UpdateStatusResponse {
  return {
    current: "1.0.0",
    latest: "1.1.0",
    update_available: true,
    last_checked: "2026-09-26T14:00:00.000Z",
    checking: false,
    rollback_notice: null,
    unverified_update: null,
    ...overrides,
  };
}

function rollbackNotice(overrides: Partial<RollbackNoticeResponse> = {}): RollbackNoticeResponse {
  return {
    attempted_version: "1.2.0",
    reason: "The new version never became ready",
    at: "2026-09-26T14:30:00.000Z",
    ...overrides,
  };
}

describe("Update page", () => {
  let gatewayUp = true;
  let status = updateStatus();

  beforeEach(() => {
    vi.useFakeTimers({ now: new Date("2026-09-26T15:00:00Z") });
    gatewayUp = true;
    status = updateStatus();
    mockFetch((url, init) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/api/update/apply") && method === "POST") {
        return jsonResponse(status);
      }
      if (url.endsWith("/api/update/status") && method === "GET") {
        if (!gatewayUp) return Promise.reject(new TypeError("Failed to fetch"));
        return jsonResponse(status);
      }
      throw new Error(`unexpected ${method} ${url}`);
    });
  });

  async function openAndApply(): Promise<void> {
    render(Update);
    await settle();
    gatewayUp = false;
    await fireEvent.click(screen.getByRole("button", { name: "Update & Restart" }));
    await settle();
  }

  it("shows elapsed time while the restarted gateway is not answering", async () => {
    await openAndApply();

    expect(screen.getByText("Restarting... (0s)")).toBeInTheDocument();
    expect(screen.getByText(/within 90s/)).toBeInTheDocument();

    await advance(RESTART_POLL_MS);

    expect(screen.getByText("Restarting... (1s)")).toBeInTheDocument();
    expect(screen.queryByText(/Updated to/)).not.toBeInTheDocument();
    expect(screen.queryByText(/failed and was rolled back/)).not.toBeInTheDocument();
  });

  it("shows the new version once status answers without a rollback", async () => {
    await openAndApply();

    status = updateStatus({
      current: "1.1.0",
      latest: "1.1.0",
      update_available: false,
    });
    gatewayUp = true;
    await advance(RESTART_POLL_MS);

    expect(screen.getByText("Updated to 1.1.0")).toBeInTheDocument();
    expect(screen.queryByText(/Restarting/)).not.toBeInTheDocument();
    expect(screen.queryByText(/failed and was rolled back/)).not.toBeInTheDocument();
  });

  it("shows the rollback reason when status reports one during the restart", async () => {
    await openAndApply();

    status = updateStatus({
      current: "1.0.0",
      latest: "1.2.0",
      update_available: false,
      rollback_notice: rollbackNotice(),
    });
    gatewayUp = true;
    await advance(RESTART_POLL_MS);

    expect(screen.getByText("Update to 1.2.0 failed and was rolled back")).toBeInTheDocument();
    expect(
      screen.getByText("The new version never became ready. Now running 1.0.0."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Updated to/)).not.toBeInTheDocument();
  });

  it("shows a rollback that was already on the status endpoint when the page opened", async () => {
    status = updateStatus({
      current: "1.0.0",
      latest: "1.2.0",
      update_available: false,
      rollback_notice: rollbackNotice({ reason: "The new version exited while starting" }),
    });

    render(Update);
    await settle();

    expect(screen.getByText("Update to 1.2.0 failed and was rolled back")).toBeInTheDocument();
    expect(
      screen.getByText("The new version exited while starting. Now running 1.0.0."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Update & Restart" })).not.toBeInTheDocument();
  });

  it("says it is still waiting once the health window passes with no response", async () => {
    await openAndApply();

    await advance(RESTART_TIMEOUT_MS + RESTART_POLL_MS);

    expect(screen.getByText("Still waiting for the gateway to come back")).toBeInTheDocument();
    expect(screen.getByText(/over 90s with no response/)).toBeInTheDocument();
    expect(screen.getByText("residuum logs")).toBeInTheDocument();
    expect(screen.queryByText(/Restarting/)).not.toBeInTheDocument();
  });
});
