import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import TeamView from "./TeamView.svelte";
import { hub } from "../lib/hub.svelte";
import { router } from "../lib/router.svelte";
import { toast } from "../lib/toast.svelte";
import { notifications } from "../lib/notifications.svelte";
import type { AgentSummary, DeletedAgent } from "../lib/hub-types";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    state: "running",
    last_error: null,
    autostart: true,
    role: `${name} keeps notes`,
    a2a_visibility: "private",
    ...overrides,
  };
}

interface Call {
  method: string;
  url: string;
  body: unknown;
}

let calls: Call[] = [];

/** What `GET /api/hub/agents/deleted` answers; the view asks for it on mount, so it is not in `calls`. */
let deletedAgents: DeletedAgent[] = [];
let deletedListFails = false;

function deletedAgent(name: string, hoursAgo = 3): DeletedAgent {
  return {
    name,
    deleted_at: new Date(Date.now() - hoursAgo * 3_600_000).toISOString(),
    checkpoint_id: `ckpt-${name}`,
  };
}

/** Answer the hub lifecycle routes; `respond` can override any of them. */
function serveHub(respond?: (call: Call) => Response | undefined): void {
  calls = [];
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    if (url === "/api/hub/agents/deleted" && method === "GET") {
      return deletedListFails
        ? jsonResponse({ error: "history unreadable" }, 500)
        : jsonResponse({ agents: deletedAgents });
    }
    const call: Call = {
      method,
      url,
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    const override = respond?.(call);
    if (override) return override;
    const lifecycle = /^\/api\/hub\/agents\/([^/]+)\/(start|stop|restart)$/.exec(url);
    if (lifecycle && method === "POST") {
      const state = lifecycle[2] === "stop" ? "stopped" : "running";
      return jsonResponse(agent(lifecycle[1] ?? "", { state }));
    }
    if (url === "/api/hub/agents" && method === "POST") {
      return jsonResponse(agent((call.body as { name: string }).name), 201);
    }
    if (url === "/api/hub/agents/restore" && method === "POST") {
      return jsonResponse(agent((call.body as { name: string }).name), 201);
    }
    const one = /^\/api\/hub\/agents\/([^/]+)$/.exec(url);
    if (one && method === "DELETE") return jsonResponse({ deleted: true, checkpoint_id: "ckpt-9" });
    if (one && method === "PATCH") {
      return jsonResponse(agent(one[1] ?? "", call.body as Partial<AgentSummary>));
    }
    return jsonResponse({ error: `unexpected ${method} ${url}` }, 500);
  });
}

beforeEach(() => {
  deletedAgents = [];
  deletedListFails = false;
  hub.handleFrame({
    type: "agents_snapshot",
    agents: [
      agent("atlas"),
      agent("brittle", {
        state: "failed",
        role: null,
        last_error: { message: "providers.toml is missing", at: "2026-09-29T10:00:00Z" },
      }),
      agent("drifter", { state: "stopped", autostart: false, a2a_visibility: "public" }),
    ],
  });
  serveHub();
});

afterEach(() => {
  hub.handleFrame({ type: "agents_snapshot", agents: [] });
  hub.notices = [];
  hub.deleted = [];
  hub.deletedLoaded = false;
  hub.deletedError = null;
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

function row(name: string): HTMLElement {
  const link = screen.getByRole("button", { name: new RegExp(`^${name}$`) });
  const li = link.closest("li");
  if (!li) throw new Error(`no row for ${name}`);
  return li;
}

function visibilityOf(name: string): HTMLSelectElement {
  const select = row(name).querySelector("select");
  if (!select) throw new Error(`no visibility select for ${name}`);
  return select;
}

function within(el: HTMLElement): { button: (name: string) => HTMLButtonElement } {
  return {
    button: (name) => {
      const found = Array.from(el.querySelectorAll("button")).find(
        (b) => b.textContent.trim() === name,
      );
      if (!found) throw new Error(`no button ${name}`);
      return found;
    },
  };
}

describe("TeamView agent list", () => {
  it("shows each agent's state, role line, visibility and last error", () => {
    render(TeamView, { onClose: () => {} });
    expect(row("atlas")).toHaveTextContent("running");
    expect(row("atlas")).toHaveTextContent("atlas keeps notes");
    expect(visibilityOf("atlas")).toHaveValue("private");
    expect(row("drifter")).toHaveTextContent("stopped");
    expect(visibilityOf("drifter")).toHaveValue("public");
    expect(row("brittle")).toHaveTextContent("failed");
    expect(row("brittle")).toHaveTextContent("providers.toml is missing");
  });

  it("shows activity: working and unread", () => {
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: true, unread: 2 });
    render(TeamView, { onClose: () => {} });
    expect(row("atlas")).toHaveTextContent("working");
    expect(row("atlas")).toHaveTextContent("2 unread");
  });

  it("only offers the lifecycle actions that fit the agent's state", () => {
    render(TeamView, { onClose: () => {} });
    const running = within(row("atlas"));
    expect(running.button("Start")).toBeDisabled();
    expect(running.button("Stop")).toBeEnabled();
    expect(running.button("Restart")).toBeEnabled();
    const stopped = within(row("drifter"));
    expect(stopped.button("Start")).toBeEnabled();
    expect(stopped.button("Stop")).toBeDisabled();
    expect(stopped.button("Restart")).toBeDisabled();
    const failed = within(row("brittle"));
    expect(failed.button("Start")).toBeEnabled();
    expect(failed.button("Restart")).toBeEnabled();
  });

  it("says why a lifecycle button is unavailable, and gives every control the agent's name", () => {
    render(TeamView, { onClose: () => {} });
    const start = within(row("atlas")).button("Start");
    expect(start).toBeDisabled();
    expect(start).toHaveAttribute("title", "atlas is already running");
    expect(start).toHaveAccessibleName("Start atlas");
    const stop = within(row("drifter")).button("Stop");
    expect(stop).toBeDisabled();
    expect(stop).toHaveAttribute("title", "drifter is not running");
    expect(within(row("atlas")).button("Stop")).not.toHaveAttribute("title");
    expect(screen.getByLabelText("A2A card for atlas")).toBe(visibilityOf("atlas"));
    expect(screen.getByLabelText("Start automatically for drifter")).not.toBeChecked();
  });

  it("keeps the whole row disabled and marks the running action while it is pending", async () => {
    mockFetch(() => new Promise<Response>(() => {}));
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(within(row("atlas")).button("Stop"));
    const pending = within(row("atlas")).button("Stopping");
    expect(pending).toBeDisabled();
    expect(pending).toHaveAccessibleName("Stopping atlas");
    expect(pending).not.toHaveAttribute("title");
    expect(row("atlas")).toHaveAttribute("aria-busy", "true");
    expect(within(row("atlas")).button("Delete")).toBeDisabled();
  });

  it("opens an agent from its name", async () => {
    const open = vi.spyOn(router, "openAgent").mockImplementation(() => {});
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: /^atlas$/ }));
    expect(open).toHaveBeenCalledWith("atlas");
  });
});

describe("TeamView lifecycle", () => {
  it("stops an agent, showing a pending label until the call returns", async () => {
    let release: (r: Response) => void = () => {};
    mockFetch(
      () =>
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
    );
    render(TeamView, { onClose: () => {} });
    const stop = within(row("atlas")).button("Stop");
    await fireEvent.click(stop);
    expect(within(row("atlas")).button("Stopping")).toBeDisabled();
    expect(within(row("atlas")).button("Restart")).toBeDisabled();
    release(jsonResponse(agent("atlas", { state: "stopped" })));
    await vi.waitFor(() => {
      expect(within(row("atlas")).button("Start")).toBeEnabled();
    });
    expect(within(row("atlas")).button("Stop")).toBeDisabled();
    expect(row("atlas")).toHaveTextContent("stopped");
  });

  it("starts and restarts through the hub API", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(within(row("drifter")).button("Start"));
    await settle();
    await fireEvent.click(within(row("atlas")).button("Restart"));
    await settle();
    expect(calls.map((c) => `${c.method} ${c.url}`)).toEqual([
      "POST /api/hub/agents/drifter/start",
      "POST /api/hub/agents/atlas/restart",
    ]);
  });

  it("surfaces a failed action as an error the user can read", async () => {
    serveHub((call) =>
      call.url.endsWith("/start")
        ? jsonResponse({ error: "drifter has no providers" }, 400)
        : undefined,
    );
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(within(row("drifter")).button("Start"));
    await vi.waitFor(() => {
      expect(within(row("drifter")).button("Start")).toBeEnabled();
    });
    const errors = [...toast.toasts.values()].filter((t) => t.kind === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0]?.message).toContain("drifter");
    expect(within(row("drifter")).button("Start")).toBeEnabled();
  });

  it("toggles autostart", async () => {
    render(TeamView, { onClose: () => {} });
    const checkbox = row("drifter").querySelector<HTMLInputElement>("input[type=checkbox]");
    expect(checkbox?.checked).toBe(false);
    await fireEvent.click(checkbox as HTMLInputElement);
    await settle();
    expect(calls).toEqual([
      { method: "PATCH", url: "/api/hub/agents/drifter", body: { autostart: true } },
    ]);
    expect(hub.agent("drifter")?.autostart).toBe(true);
  });

  it("puts the autostart box back when the change fails", async () => {
    serveHub(() => jsonResponse({ error: "nope" }, 500));
    render(TeamView, { onClose: () => {} });
    const checkbox = row("drifter").querySelector<HTMLInputElement>("input[type=checkbox]");
    await fireEvent.click(checkbox as HTMLInputElement);
    await vi.waitFor(() => {
      expect(checkbox?.checked).toBe(false);
      expect(checkbox).toBeEnabled();
    });
  });
});

describe("TeamView A2A visibility", () => {
  it("explains that public exposes only the card", () => {
    render(TeamView, { onClose: () => {} });
    expect(screen.getByText(/Public shows only an agent's card/)).toBeTruthy();
    expect(screen.getByText(/still needs a caller key/)).toBeTruthy();
    expect(visibilityOf("atlas")).toHaveAttribute("aria-describedby", "team-visibility-hint");
  });

  it("changes visibility through the hub API and keeps the choice", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.change(visibilityOf("atlas"), { target: { value: "public" } });
    await vi.waitFor(() => {
      expect(hub.agent("atlas")?.a2a_visibility).toBe("public");
    });
    expect(calls).toEqual([
      { method: "PATCH", url: "/api/hub/agents/atlas", body: { a2a_visibility: "public" } },
    ]);
    expect(visibilityOf("atlas")).toHaveValue("public");
  });

  it("puts the choice back and reports the error when the change fails", async () => {
    serveHub(() => jsonResponse({ error: "nope" }, 500));
    render(TeamView, { onClose: () => {} });
    await fireEvent.change(visibilityOf("atlas"), { target: { value: "public" } });
    await vi.waitFor(() => {
      expect(visibilityOf("atlas")).toHaveValue("private");
      expect(visibilityOf("atlas")).toBeEnabled();
    });
    expect([...toast.toasts.values()].some((t) => t.kind === "error")).toBe(true);
  });

  it("is disabled while the change is in flight", async () => {
    let release: (r: Response) => void = () => {};
    mockFetch(
      () =>
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
    );
    render(TeamView, { onClose: () => {} });
    await fireEvent.change(visibilityOf("atlas"), { target: { value: "public" } });
    expect(visibilityOf("atlas")).toBeDisabled();
    release(jsonResponse(agent("atlas", { a2a_visibility: "public" })));
    await vi.waitFor(() => {
      expect(visibilityOf("atlas")).toBeEnabled();
    });
  });
});

describe("TeamView delete", () => {
  it("asks first, saying the directory is removed and can be restored from checkpoints", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("removes");
    expect(dialog).toHaveTextContent("directory");
    expect(dialog).toHaveTextContent("undo this and restore it");
    expect(calls).toEqual([]);
  });

  it("does nothing when the dialog is cancelled", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(calls).toEqual([]);
  });

  it("deletes after confirming and shows the checkpoint id", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    await fireEvent.click(screen.getByRole("button", { name: "Delete agent" }));
    expect((await screen.findByText("ckpt-9")).closest("[role=status]")).toBeTruthy();
    expect(calls).toEqual([{ method: "DELETE", url: "/api/hub/agents/drifter", body: undefined }]);
    expect(screen.queryByRole("button", { name: "Delete drifter" })).toBeNull();
  });

  it("says plainly when no checkpoint was taken, with nothing to undo", async () => {
    serveHub((call) =>
      call.method === "DELETE" ? jsonResponse({ deleted: true, checkpoint_id: null }) : undefined,
    );
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    await fireEvent.click(screen.getByRole("button", { name: "Delete agent" }));
    expect(await screen.findByText(/No checkpoint was taken/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Undo deleting drifter" })).toBeNull();
  });

  it("offers Undo on the deletion note, which restores the agent from its checkpoint", async () => {
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    await fireEvent.click(screen.getByRole("button", { name: "Delete agent" }));
    await screen.findByText("ckpt-9");
    expect(screen.queryByRole("button", { name: "drifter" })).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Undo deleting drifter" }));

    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "drifter" })).toBeTruthy();
    });
    expect(calls.at(-1)).toEqual({
      method: "POST",
      url: "/api/hub/agents/restore",
      body: { name: "drifter", checkpoint_id: "ckpt-9" },
    });
    expect(screen.queryByText("ckpt-9")).toBeNull();
  });

  it("shows Undo as pending and keeps the note when the restore fails", async () => {
    let release: (r: Response) => void = () => {};
    render(TeamView, { onClose: () => {} });
    await fireEvent.click(screen.getByRole("button", { name: "Delete drifter" }));
    await fireEvent.click(screen.getByRole("button", { name: "Delete agent" }));
    await screen.findByText("ckpt-9");
    mockFetch(
      () =>
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
    );

    await fireEvent.click(screen.getByRole("button", { name: "Undo deleting drifter" }));

    const undo = screen.getByRole("button", { name: "Undo deleting drifter" });
    expect(undo).toBeDisabled();
    expect(undo).toHaveTextContent("Restoring");
    release(jsonResponse({ error: "an agent named 'drifter' already exists" }, 409));
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Undo deleting drifter" })).toBeEnabled();
    });
    expect([...toast.toasts.values()].some((t) => t.kind === "error")).toBe(true);
    expect(screen.getByText("ckpt-9")).toBeTruthy();
  });
});

describe("TeamView recently deleted", () => {
  const restoreButton = (name: string): HTMLElement =>
    screen.getByRole("button", { name: `Restore ${name}` });

  it("has no section when nothing is deleted", async () => {
    render(TeamView, { onClose: () => {} });
    await vi.waitFor(() => {
      expect(hub.deletedLoaded).toBe(true);
    });
    expect(screen.queryByText("Recently deleted")).toBeNull();
  });

  it("lists each deleted agent with when it was deleted", async () => {
    deletedAgents = [deletedAgent("nova", 3), deletedAgent("kit", 48)];
    render(TeamView, { onClose: () => {} });

    expect(await screen.findByText("Recently deleted")).toBeTruthy();
    const nova = restoreButton("nova").closest("li");
    const kit = restoreButton("kit").closest("li");
    expect(nova).toHaveTextContent("nova");
    expect(nova).toHaveTextContent("deleted 3h ago");
    expect(kit).toHaveTextContent("deleted 2d ago");
  });

  it("restores from the deletion's checkpoint, shows progress, and moves the agent into the list", async () => {
    deletedAgents = [deletedAgent("nova")];
    render(TeamView, { onClose: () => {} });
    await screen.findByText("Recently deleted");
    let release: (r: Response) => void = () => {};
    mockFetch(
      () =>
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
    );

    await fireEvent.click(restoreButton("nova"));

    expect(restoreButton("nova")).toBeDisabled();
    expect(restoreButton("nova")).toHaveTextContent("Restoring");
    release(jsonResponse(agent("nova"), 201));
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "nova" })).toBeTruthy();
    });
    expect(screen.queryByText("Recently deleted")).toBeNull();
    expect(hub.deleted).toEqual([]);
  });

  it("sends the checkpoint id of the row", async () => {
    deletedAgents = [deletedAgent("nova")];
    render(TeamView, { onClose: () => {} });
    await screen.findByText("Recently deleted");

    await fireEvent.click(restoreButton("nova"));

    await vi.waitFor(() => {
      expect(calls).toEqual([
        {
          method: "POST",
          url: "/api/hub/agents/restore",
          body: { name: "nova", checkpoint_id: "ckpt-nova" },
        },
      ]);
    });
  });

  it("keeps the row and surfaces the reason when the restore is refused", async () => {
    deletedAgents = [deletedAgent("nova")];
    serveHub((call) =>
      call.url === "/api/hub/agents/restore"
        ? jsonResponse({ error: "an agent named 'nova' already exists" }, 409)
        : undefined,
    );
    render(TeamView, { onClose: () => {} });
    await screen.findByText("Recently deleted");

    await fireEvent.click(restoreButton("nova"));

    await vi.waitFor(() => {
      expect(restoreButton("nova")).toBeEnabled();
    });
    const errors = [...toast.toasts.values()].filter((t) => t.kind === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0]?.message).toContain("Couldn't restore nova");
    expect(errors[0]?.message).toContain("already exists");
  });

  it("says when the list can't be loaded and loads it on Try again", async () => {
    deletedAgents = [deletedAgent("nova")];
    deletedListFails = true;
    render(TeamView, { onClose: () => {} });
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't load the deleted agents");

    deletedListFails = false;
    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByRole("button", { name: "Restore nova" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("TeamView create form", () => {
  const nameInput = (): HTMLInputElement => screen.getByLabelText<HTMLInputElement>("Name");
  const createButton = (): HTMLElement => screen.getByRole("button", { name: "Create agent" });

  async function typeName(value: string): Promise<void> {
    await fireEvent.input(nameInput(), { target: { value } });
    await fireEvent.blur(nameInput());
  }

  it.each([
    ["Upper", "Use only lowercase letters, digits, and hyphens."],
    ["has_underscore", "Use only lowercase letters, digits, and hyphens."],
    ["-lead", "The name can't start or end with a hyphen."],
    ["trail-", "The name can't start or end with a hyphen."],
    ["a".repeat(25), "Use 24 characters or fewer."],
    ["hub", '"hub" is reserved. Pick a different name.'],
    ["team", '"team" is reserved. Pick a different name.'],
    ["agents", '"agents" is reserved. Pick a different name.'],
    ["atlas", 'An agent named "atlas" already exists.'],
  ])("explains why %s is not allowed and blocks creating it", async (name, message) => {
    render(TeamView, { onClose: () => {} });
    await typeName(name);
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(nameInput()).toHaveAttribute("aria-invalid", "true");
    expect(createButton()).toBeDisabled();
  });

  it("accepts a valid name", async () => {
    render(TeamView, { onClose: () => {} });
    await typeName("nova-2");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(createButton()).toBeEnabled();
  });

  it("does not scold an empty form, but does not allow creating from it", () => {
    render(TeamView, { onClose: () => {} });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(createButton()).toBeDisabled();
  });

  it("explains what the description becomes", () => {
    render(TeamView, { onClose: () => {} });
    expect(screen.getByText(/turns the description into its own notes/)).toBeTruthy();
  });

  it("defaults to private visibility and copying models from the first agent", () => {
    render(TeamView, { onClose: () => {} });
    expect(screen.getByLabelText(/Private/)).toBeChecked();
    expect(screen.getByLabelText("Copy model settings from")).toHaveValue("atlas");
  });

  it("creates with the chosen options and confirms", async () => {
    render(TeamView, { onClose: () => {} });
    await typeName("nova");
    await fireEvent.input(screen.getByLabelText(/Description/), {
      target: { value: "Watches the release feed." },
    });
    await fireEvent.change(screen.getByLabelText("Copy model settings from"), {
      target: { value: "drifter" },
    });
    await fireEvent.click(screen.getByLabelText(/Public/));
    await fireEvent.click(createButton());
    expect(await screen.findByText(/Created nova\./)).toBeTruthy();
    expect(calls).toEqual([
      {
        method: "POST",
        url: "/api/hub/agents",
        body: {
          name: "nova",
          description: "Watches the release feed.",
          models_from: "drifter",
          a2a_visibility: "public",
        },
      },
    ]);
    expect(nameInput().value).toBe("");
  });

  it("leaves the description out when it is blank", async () => {
    render(TeamView, { onClose: () => {} });
    await typeName("nova");
    await fireEvent.click(createButton());
    await settle();
    expect(calls[0]?.body).toEqual({
      name: "nova",
      models_from: "atlas",
      a2a_visibility: "private",
    });
  });

  it("keeps the form and shows the error when the hub refuses", async () => {
    serveHub(() => jsonResponse({ error: "already exists" }, 409));
    render(TeamView, { onClose: () => {} });
    await typeName("nova");
    await fireEvent.click(createButton());
    await settle();
    expect(nameInput().value).toBe("nova");
    expect([...toast.toasts.values()].some((t) => t.kind === "error")).toBe(true);
    expect(screen.queryByText(/Created nova/)).toBeNull();
  });
});
