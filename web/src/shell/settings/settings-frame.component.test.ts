import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import { ALL_SCOPE, type SectionId } from "../../lib/settings-sections";
import { toast } from "../../lib/toast.svelte";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import SettingsModal from "../SettingsModal.svelte";
import { conflictQuestion } from "./changed-on-disk.svelte";

let agent = "";
let server: FakeAgentConfig;
let count = 0;

const writes = (): string[] =>
  server.requests
    .filter((request) => request.method === "PUT" || request.method === "PATCH")
    .map((request) => `${request.method} ${request.url}`);
const toasts = (): string[] => [...toast.toasts.values()].map((shown) => shown.message);
const timeout = (): HTMLInputElement => screen.getByLabelText("Timeout (seconds)");
const saveBar = (): HTMLElement | null => screen.queryByRole("region", { name: "Unsaved changes" });

async function open(scope: string, section: SectionId | null): Promise<void> {
  render(SettingsModal);
  await router.openSettings({ scope, section });
  await settle();
  await vi.waitFor(() => {
    expect(document.querySelector(".settings-content")).not.toBeNull();
    expect(screen.queryByText("Loading settings")).toBeNull();
  });
}

async function type(input: HTMLInputElement, value: string): Promise<void> {
  await fireEvent.input(input, { target: { value } });
  await settle();
}

beforeEach(() => {
  stubWebSocket();
  vi.stubGlobal("matchMedia", (media: string) => ({
    media,
    matches: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `frame-${String(++count)}`;
  server = fakeAgentConfig(agent, { config: "timeout_secs = 30\n", hub: 'timezone = "UTC"\n' });
  mockFetch(server.handler);
});

afterEach(async () => {
  conflictQuestion.answer(null);
  await router.replacePlace({ kind: "home" });
  for (const scope of settingsModel.stagedScopes) scope.discard();
});

describe("the Settings modal frame", () => {
  it("lists the scope's sections with the Advanced group under a heading", async () => {
    await open(agent, "runtime");
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    expect(nav.textContent).toContain("Only affects");
    const advanced = screen.getByRole("group", { name: "Advanced" });
    expect([...advanced.querySelectorAll("button")].map((b) => b.dataset.section)).toEqual([
      "runtime",
      "servers",
      "a2a",
      "raw",
      "history",
    ]);
    expect(screen.getByRole("button", { name: /Runtime/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Runtime" })).toBeTruthy();
  });

  it("stages an edit, saves only the agent's files, and offers Undo", async () => {
    await open(agent, "runtime");
    expect(saveBar()).toBeNull();
    await type(timeout(), "60");
    expect(saveBar()).toHaveTextContent("You have unsaved changes.");
    expect(writes()).toEqual([]);

    await fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await vi.waitFor(() => {
      expect(saveBar()).toBeNull();
    });
    expect(writes()).toEqual([`PATCH /api/agents/${agent}/config/patch`]);
    const saved = [...toast.toasts.values()].at(-1);
    expect(saved?.message).toBe("Saved config.toml.");
    expect(saved?.action?.label).toBe("Undo");
  });

  it("brings the file's values back on Discard", async () => {
    await open(agent, "runtime");
    await type(timeout(), "60");
    await fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await settle();
    expect(timeout().value).toBe("30");
    expect(saveBar()).toBeNull();
    expect(toasts()).toContain("Discarded your changes.");
  });

  it("keeps the staged changes of a scope while another is shown", async () => {
    await open(agent, "runtime");
    await type(timeout(), "60");
    await router.switchSettingsScope(ALL_SCOPE);
    await vi.waitFor(() => {
      expect(screen.getByRole("heading", { name: "General" })).toBeTruthy();
    });
    expect(saveBar()).toBeNull();
    // Runtime isn't an All agents section, so the switch landed on General and comes back on Model.
    await router.switchSettingsScope(agent);
    await vi.waitFor(() => {
      expect(screen.getByRole("heading", { name: "Model" })).toBeTruthy();
    });
    expect(saveBar()).toBeTruthy();
    await router.switchSettingsSection("runtime");
    await vi.waitFor(() => {
      expect(timeout().value).toBe("60");
    });
  });

  it("shows a refused save at the top of the section and keeps the change staged", async () => {
    await open(agent, "runtime");
    const refuse = server.handler;
    mockFetch((url, init) =>
      init?.method === "PATCH"
        ? jsonResponse({ valid: false, error: "timeout_secs is too long", diagnostics: [] }, 400)
        : refuse(url, init),
    );
    await type(timeout(), "999999");
    await fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await vi.waitFor(() => {
      expect(saveBar()).toHaveTextContent("Couldn't save config.toml: timeout_secs is too long.");
    });
    expect(
      screen.getAllByRole("alert").some((a) => a.textContent.includes("timeout_secs is too long")),
    ).toBe(true);
    expect(timeout().value).toBe("999999");
  });

  it("asks when the file changed on disk under the change, and keeps it staged when the question is closed", async () => {
    await open(agent, "runtime");
    await type(timeout(), "60");
    server.files.config = "timeout_secs = 45\n";
    await fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("dialog", { name: `${agent}'s config.toml changed` })).toBeTruthy();
    });
    expect(screen.getByRole("button", { name: "Use what's on disk" })).toBeTruthy();

    conflictQuestion.answer(null);
    await vi.waitFor(() => {
      expect(saveBar()).toHaveTextContent("you closed the question about it");
    });
    expect(timeout().value).toBe("60");
    expect(writes()).toEqual([]);
  });

  it("writes the hub's file, and nothing of an agent's, from All agents", async () => {
    await open(ALL_SCOPE, "raw");
    const text = screen.getByRole("textbox", { name: "Contents of config.toml" });
    await type(text as HTMLInputElement, 'timezone = "Europe/Berlin"\n');
    await fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    await vi.waitFor(() => {
      expect(writes()).toEqual(["PUT /api/hub/config/raw"]);
    });
  });

  it("keeps a file's raw editor read-only while its form holds staged changes", async () => {
    await open(agent, "runtime");
    await type(timeout(), "60");
    await router.switchSettingsSection("raw");
    await vi.waitFor(() => {
      expect(screen.getByText("Save or discard your form changes first.")).toBeTruthy();
    });
    expect(screen.getByRole("textbox", { name: "Contents of config.toml" })).toHaveAttribute(
      "readonly",
    );
  });
});
