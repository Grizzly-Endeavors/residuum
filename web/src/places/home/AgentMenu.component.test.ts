import userEvent, { type UserEvent } from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import { confirmations, type StatusDotState } from "../../lib/ui";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { snapshot } from "../../test/hub-frames";
import AgentMenu from "./AgentMenu.svelte";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
    ...overrides,
  };
}

interface Call {
  method: string;
  url: string;
  body: unknown;
}

let calls: Call[] = [];
let patchFails = false;
let checkpointId: string | null = "ckpt-9";

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  calls = [];
  patchFails = false;
  checkpointId = "ckpt-9";
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    const body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    calls.push({ method, url, body });
    const lifecycle = /^\/api\/hub\/agents\/([^/]+)\/(start|stop|restart)$/.exec(url);
    if (lifecycle) {
      return jsonResponse(
        agent(lifecycle[1] ?? "", { state: lifecycle[2] === "stop" ? "stopped" : "running" }),
      );
    }
    const one = /^\/api\/hub\/agents\/([^/]+)$/.exec(url);
    if (one && method === "PATCH") {
      if (patchFails) return jsonResponse({ error: "config.toml is read-only" }, 500);
      return jsonResponse(agent(one[1] ?? "", body as Partial<AgentSummary>));
    }
    if (one && method === "DELETE") {
      return jsonResponse({ deleted: true, checkpoint_id: checkpointId });
    }
    return jsonResponse({ error: `unexpected ${method} ${url}` }, 500);
  });
});

afterEach(() => {
  hub.handleFrame(snapshot([]));
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

/**
 * Show the menu of an agent the hub knows, in the state the board gives it.
 * `follow` hands it the hub's summary again, as the board does when the list changes.
 */
async function openMenu(
  summary: AgentSummary,
  state: StatusDotState = summary.state,
): Promise<{ user: UserEvent; follow: () => Promise<void> }> {
  const user = userEvent.setup();
  hub.handleFrame(snapshot([summary]));
  const { rerender } = render(AgentMenu, { agent: summary, state, working: false });
  await user.click(screen.getByRole("button", { name: `Manage ${summary.name}` }));
  const follow = async (): Promise<void> => {
    await settle();
    await rerender({ agent: hub.agent(summary.name) ?? summary });
  };
  return { user, follow };
}

function item(name: string): HTMLElement {
  return screen.getByRole("menuitem", { name });
}

describe("AgentMenu", () => {
  it("names the agent's state, and offers only what that state takes", async () => {
    await openMenu(agent("drifter", { state: "stopped" }));
    expect(screen.getByRole("menu", { name: "Manage drifter" })).toHaveAccessibleDescription(
      "drifter, stopped",
    );
    expect(item("Start")).not.toHaveAttribute("aria-disabled");
    expect(item("Stop")).toHaveAttribute("aria-disabled", "true");
    expect(item("Restart")).toHaveAttribute("aria-disabled", "true");
    expect(item("Open chat")).not.toHaveAttribute("aria-disabled");
    expect(item("Delete")).not.toHaveAttribute("aria-disabled");
  });

  it("offers nothing to start or stop while the agent stops", async () => {
    await openMenu(agent("atlas"), "stopping");
    for (const name of ["Start", "Stop", "Restart"]) {
      expect(item(name)).toHaveAttribute("aria-disabled", "true");
    }
  });

  it("starts, stops and restarts through the hub", async () => {
    const { user } = await openMenu(agent("atlas"));
    await user.click(item("Stop"));
    await settle();
    expect(calls).toContainEqual({
      method: "POST",
      url: "/api/hub/agents/atlas/stop",
      body: undefined,
    });
    expect(hub.agent("atlas")?.state).toBe("stopped");
  });

  it("flips Start automatically, and puts it back when the hub refuses", async () => {
    const { user, follow } = await openMenu(agent("atlas", { autostart: true }));
    const autostart = screen.getByRole("menuitemcheckbox", { name: "Start automatically" });
    expect(autostart).toHaveAttribute("aria-checked", "true");

    await user.click(autostart);
    await follow();
    expect(calls.at(-1)).toEqual({
      method: "PATCH",
      url: "/api/hub/agents/atlas",
      body: { autostart: false },
    });
    expect(autostart).toHaveAttribute("aria-checked", "false");

    patchFails = true;
    await user.click(autostart);
    await follow();
    expect(autostart).toHaveAttribute("aria-checked", "false");
    expect(notifications.history[0]?.message).toMatch(/^Couldn't change the autostart setting/);
  });

  it("asks before deleting, and deletes nothing when the answer is no", async () => {
    const { user } = await openMenu(agent("drifter", { state: "stopped" }));
    await user.click(item("Delete"));
    expect(confirmations.current).toMatchObject({
      title: "Delete drifter?",
      confirmLabel: "Delete drifter",
      tone: "danger",
    });
    expect(confirmations.current?.message).toMatch(/^drifter's folder is removed/);
    confirmations.answer(false);
    await vi.waitFor(() => {
      expect(confirmations.current).toBeNull();
    });
    await settle();
    expect(calls.filter((call) => call.method === "DELETE")).toEqual([]);
  });

  it("says a running agent stops first, then deletes it and drops its staged settings", async () => {
    const drop = vi.spyOn(settingsModel, "drop");
    const { user } = await openMenu(agent("atlas"));
    await user.click(item("Delete"));
    expect(confirmations.current?.message).toMatch(/^atlas stops, and its folder is removed/);
    confirmations.answer(true);

    await vi.waitFor(() => {
      expect(drop).toHaveBeenCalledWith("atlas");
    });
    expect(calls).toContainEqual({
      method: "DELETE",
      url: "/api/hub/agents/atlas",
      body: undefined,
    });
    expect(hub.agent("atlas")).toBeUndefined();
    expect(notifications.history).toEqual([]);
  });

  it("warns when a deletion took no checkpoint", async () => {
    checkpointId = null;
    const { user } = await openMenu(agent("drifter", { state: "stopped" }));
    await user.click(item("Delete"));
    confirmations.answer(true);
    await vi.waitFor(() => {
      expect(notifications.history[0]).toMatchObject({ kind: "error" });
    });
    expect(notifications.history[0]?.message).toMatch(/no checkpoint was taken first/);
  });
});
