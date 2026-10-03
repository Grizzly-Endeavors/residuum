import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary, DeletedAgent } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import { toast } from "../../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen } from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import RecentlyDeleted from "./RecentlyDeleted.svelte";

const NOW = Date.parse("2026-03-14T12:00:00Z");

function deletedAgent(name: string, hoursAgo: number): DeletedAgent {
  return {
    name,
    display_name: name,
    deleted_at: new Date(NOW - hoursAgo * 3_600_000).toISOString(),
    checkpoint_id: `ckpt-${name}`,
  };
}

function restored(name: string): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

let deleted: DeletedAgent[] = [];
let listFails = false;
let restores: unknown[] = [];

beforeEach(() => {
  deleted = [];
  listFails = false;
  restores = [];
  hub.handleFrame(snapshot([]));
  mockFetch((url, init) => {
    if (url === "/api/hub/agents/deleted") {
      return listFails
        ? jsonResponse({ error: "history unreadable" }, 500)
        : jsonResponse({ agents: deleted });
    }
    if (url === "/api/hub/agents/restore" && init?.method === "POST") {
      const body = JSON.parse(typeof init.body === "string" ? init.body : "{}") as {
        name: string;
      };
      restores.push(body);
      deleted = deleted.filter((gone) => gone.name !== body.name);
      return jsonResponse(restored(body.name), 201);
    }
    return jsonResponse({ error: `unexpected ${url}` }, 500);
  });
});

afterEach(() => {
  hub.deleted = [];
  hub.deletedLoaded = false;
  hub.deletedError = null;
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

/** The disclosure's button, once the list has loaded. */
function disclosure(): Promise<HTMLElement> {
  return screen.findByRole("button", { name: "Recently deleted" });
}

describe("RecentlyDeleted", () => {
  it("shows nothing while no agent can be restored", async () => {
    render(RecentlyDeleted, { now: NOW });
    await vi.waitFor(() => {
      expect(hub.deletedLoaded).toBe(true);
    });
    expect(screen.queryByRole("button", { name: "Recently deleted" })).not.toBeInTheDocument();
  });

  it("is collapsed, and lists each deleted agent with when it went and Restore", async () => {
    const user = userEvent.setup();
    deleted = [deletedAgent("drifter", 2), deletedAgent("nova", 30)];
    render(RecentlyDeleted, { now: NOW });

    expect(await disclosure()).toHaveAttribute("aria-expanded", "false");
    await user.click(await disclosure());
    const rows = screen.getAllByRole("listitem");
    expect(rows.map((row) => row.textContent.replace(/\s+/g, " ").trim())).toEqual([
      "drifter Deleted 2h ago Restore",
      "nova Deleted 1d ago Restore",
    ]);
  });

  it("restores from the deletion's checkpoint, and keeps focus on the disclosure", async () => {
    const user = userEvent.setup();
    deleted = [deletedAgent("drifter", 2)];
    render(RecentlyDeleted, { now: NOW });
    await user.click(await disclosure());
    await user.click(screen.getByRole("button", { name: "Restore drifter" }));

    await screen.findByText("Nothing to restore.");
    expect(restores).toEqual([{ name: "drifter", checkpoint_id: "ckpt-drifter" }]);
    expect(hub.agent("drifter")).toBeDefined();
    await vi.waitFor(async () => {
      expect(await disclosure()).toHaveFocus();
    });
  });

  it("says when the list can't load, and Try again loads it", async () => {
    const user = userEvent.setup();
    listFails = true;
    render(RecentlyDeleted, { now: NOW });
    await user.click(await disclosure());
    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't load the deleted agents\./);

    listFails = false;
    deleted = [deletedAgent("drifter", 2)];
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("button", { name: "Restore drifter" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
