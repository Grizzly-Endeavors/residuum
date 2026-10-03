import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary } from "../lib/hub-types";
import { hub } from "../lib/hub.svelte";
import { notifications } from "../lib/notifications.svelte";
import { router } from "../lib/router.svelte";
import { toast } from "../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import { snapshot } from "../test/hub-frames";
import CreateAgentDialog from "./CreateAgentDialog.svelte";

function agent(name: string): AgentSummary {
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

/** The bodies of the create requests sent. */
let created: Record<string, unknown>[] = [];
let createFails = false;

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  created = [];
  createFails = false;
  // The dialog is a sheet only at phone width.
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: false,
    media,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  hub.handleFrame(snapshot([agent("atlas"), agent("scout")]));
  mockFetch((url, init) => {
    if (url === "/api/hub/agents" && init?.method === "POST") {
      const body = JSON.parse(typeof init.body === "string" ? init.body : "{}") as Record<
        string,
        unknown
      >;
      created.push(body);
      if (createFails) return jsonResponse({ error: "the disk is full" }, 500);
      return jsonResponse(agent(String(body.name)), 201);
    }
    return jsonResponse({ error: `unexpected ${url}` }, 500);
  });
});

afterEach(() => {
  hub.handleFrame(snapshot([]));
  hub.deleted = [];
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

function nameField(): HTMLElement {
  return screen.getByRole("textbox", { name: "Name" });
}

function openDialog(oncreated = vi.fn()): ReturnType<typeof vi.fn> {
  render(CreateAgentDialog, { open: true, oncreated });
  return oncreated;
}

describe("CreateAgentDialog", () => {
  it("opens on the name field", () => {
    openDialog();
    expect(screen.getByRole("dialog", { name: "Create an agent" })).toBeInTheDocument();
    expect(nameField()).toHaveFocus();
  });

  it("checks the name as it is typed, and sends nothing until it passes", async () => {
    const user = userEvent.setup();
    openDialog();
    await user.type(nameField(), "Atlas!");
    expect(nameField()).toHaveAccessibleDescription(/letters, numbers, spaces/);
    expect(nameField()).toHaveAttribute("aria-invalid", "true");

    await user.clear(nameField());
    await user.type(nameField(), "scout");
    expect(nameField()).toHaveAccessibleDescription(/You already have an agent called scout\./);

    await user.clear(nameField());
    await user.type(nameField(), "team");
    expect(nameField()).toHaveAccessibleDescription(/"team" is reserved/);
    await user.click(screen.getByRole("button", { name: "Create agent" }));
    expect(created).toEqual([]);
    expect(nameField()).toHaveFocus();
  });

  it("says a name is needed once Create is pressed on an empty one", async () => {
    const user = userEvent.setup();
    openDialog();
    expect(nameField()).not.toHaveAttribute("aria-invalid");
    await user.click(screen.getByRole("button", { name: "Create agent" }));
    expect(nameField()).toHaveAccessibleDescription(/Give your agent a name\./);
    expect(created).toEqual([]);
  });

  it("creates a private agent with the first agent's model settings, then closes", async () => {
    const user = userEvent.setup();
    const oncreated = openDialog();
    await user.type(nameField(), "research-buddy");
    await user.type(
      screen.getByRole("textbox", { name: "What should it help with?" }),
      "  Keep my reading list  ",
    );
    await user.click(screen.getByRole("button", { name: "Create agent" }));
    await settle();

    expect(created).toEqual([
      {
        name: "research-buddy",
        description: "Keep my reading list",
        models_from: "atlas",
        a2a_visibility: "private",
      },
    ]);
    expect(oncreated).toHaveBeenCalledWith("research-buddy");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(hub.agent("research-buddy")).toBeDefined();
  });

  it("takes the model settings and visibility chosen under More options", async () => {
    const user = userEvent.setup();
    openDialog();
    await user.type(nameField(), "nova");
    await user.click(screen.getByRole("button", { name: "More options" }));
    await user.selectOptions(
      screen.getByRole("combobox", { name: "Copy model settings from" }),
      "scout",
    );
    const visibility = screen.getByRole("combobox", { name: "Who can find it" });
    expect(visibility).toHaveAccessibleDescription(/need a caller key even to see it/);
    await user.selectOptions(visibility, "public");
    expect(visibility).toHaveAccessibleDescription(/Anyone with its address/);
    await user.click(screen.getByRole("button", { name: "Create agent" }));
    await settle();

    expect(created[0]).toMatchObject({ models_from: "scout", a2a_visibility: "public" });
  });

  it("keeps what was typed when the hub refuses, and says why", async () => {
    const user = userEvent.setup();
    createFails = true;
    const oncreated = openDialog();
    await user.type(nameField(), "nova");
    await user.click(screen.getByRole("button", { name: "Create agent" }));
    await settle();

    expect(oncreated).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Create an agent" })).toBeInTheDocument();
    expect(nameField()).toHaveValue("nova");
    expect(notifications.history[0]?.message).toMatch(/^Couldn't create nova\./);
  });

  it("points at Recently deleted when the name belonged to a deleted agent", async () => {
    const user = userEvent.setup();
    hub.deleted = [
      {
        name: "drifter",
        display_name: "drifter",
        deleted_at: "2026-03-14T10:00:00Z",
        checkpoint_id: "c-1",
      },
    ];
    openDialog();
    await user.type(nameField(), "drifter");
    expect(nameField()).toHaveAccessibleDescription(/restore it from Recently deleted on Home/);
    expect(nameField()).not.toHaveAttribute("aria-invalid");
  });
});
