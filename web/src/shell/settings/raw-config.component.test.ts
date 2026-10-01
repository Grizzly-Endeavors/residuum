import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { agentConfigFile, configCoordinator } from "../../lib/config-coordinator";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import type { SectionId } from "../../lib/settings-sections";
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

// The Raw config section in the Settings modal: problems found as the text is
// typed, Save and Discard, the lock in both directions, and a file that
// changed on disk under a draft.

const BROKEN = "timeout_secs = 30\nmodel = \n";
const PROBLEM = {
  severity: "error",
  message: "invalid value",
  location: { kind: "line_column", line: 2, column: 9 },
};

let agent = "";
let server: FakeAgentConfig;
let count = 0;
let checksFail = false;

const text = (): HTMLTextAreaElement =>
  screen.getByRole("textbox", { name: "Contents of config.toml" });
const toasts = (): string[] => [...toast.toasts.values()].map((shown) => shown.message);

async function open(section: SectionId): Promise<void> {
  render(SettingsModal);
  await router.openSettings({ scope: agent, section });
  await settle();
  await vi.waitFor(() => {
    expect(document.querySelector(".settings-content")).not.toBeNull();
    expect(screen.queryByText("Loading settings")).toBeNull();
  });
}

async function type(element: HTMLInputElement | HTMLTextAreaElement, value: string): Promise<void> {
  await fireEvent.input(element, { target: { value } });
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
  vi.spyOn(console, "error").mockImplementation(() => {});
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `raw-${String(++count)}`;
  checksFail = false;
  server = fakeAgentConfig(agent, { config: "timeout_secs = 30\n" });
  mockFetch((url, init) => {
    if (url === `/api/agents/${agent}/config/validate`) {
      if (checksFail) return jsonResponse({ error: "the agent is gone" }, 500);
      const broken = init?.body === BROKEN;
      return jsonResponse(broken ? { valid: false, diagnostics: [PROBLEM] } : { valid: true });
    }
    if (init?.method === "PUT" && url === `/api/agents/${agent}/config/raw`) {
      void server.handler(url, init);
      return jsonResponse({ valid: false, diagnostics: [PROBLEM] });
    }
    return server.handler(url, init);
  });
});

afterEach(async () => {
  conflictQuestion.answer(null);
  await router.replacePlace({ kind: "home" });
  settingsModel.drop(agent);
});

describe("Raw config", () => {
  it("checks the text as it is typed and lists problems at their line, marked beside it", async () => {
    await open("raw");
    await vi.waitFor(() => {
      expect(screen.getByText("No problems found.")).toBeTruthy();
    });
    await type(text(), BROKEN);
    expect(screen.getByText("Checking for problems…")).toBeTruthy();
    await vi.waitFor(() => {
      expect(screen.getByText("1 problem found.")).toBeTruthy();
    });
    const problems = screen.getByRole("list", { name: "Problems in config.toml" });
    expect(problems).toHaveTextContent("line 2, column 9 invalid value");
    expect(document.querySelector('[data-problem="error"]')?.textContent).toBe("2");

    await fireEvent.click(screen.getByRole("button", { name: "Go to line 2, column 9" }));
    expect(document.activeElement).toBe(text());
    expect(text().selectionStart).toBe("timeout_secs = 30\n".length + 8);
  });

  it("says so when the text can't be checked", async () => {
    checksFail = true;
    await open("raw");
    await vi.waitFor(() => {
      expect(screen.getByText(/Couldn't check this file for problems\./)).toBeTruthy();
    });
  });

  it("saves the whole text even with problems, and shows them", async () => {
    await open("raw");
    await type(text(), BROKEN);
    await fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    await vi.waitFor(() => {
      expect(toasts()).toContain("Saved config.toml. It has problems, listed under the editor.");
    });
    expect(server.files.config).toBe(BROKEN);
    expect(screen.getByText("1 problem found.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Discard edits" })).toBeNull();
    expect(screen.getByRole("button", { name: "Save config.toml" })).toBeDisabled();
  });

  it("keeps the form read-only while an edit here is unsaved, until it is discarded", async () => {
    await open("raw");
    await type(text(), "timeout_secs = 45\n");
    await router.switchSettingsSection("runtime");
    await vi.waitFor(() => {
      expect(screen.getByText(/You have unsaved edits to config\.toml in Raw config/)).toBeTruthy();
    });
    expect(screen.getByLabelText("Timeout (seconds)")).toBeDisabled();

    await fireEvent.click(screen.getByRole("button", { name: "Open Raw config" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("tab", { name: "config.toml (unsaved)" })).toBeTruthy();
    });
    await fireEvent.click(screen.getByRole("button", { name: "Discard edits" }));
    await router.switchSettingsSection("runtime");
    await vi.waitFor(() => {
      expect(screen.getByLabelText("Timeout (seconds)")).not.toBeDisabled();
    });
  });

  it("asks which version to keep when the file changed on disk under an edit", async () => {
    await open("raw");
    await type(text(), "timeout_secs = 45\n");
    server.files.config = "timeout_secs = 90\n";
    await configCoordinator.externalChange(agentConfigFile(agent, "config"));
    await vi.waitFor(() => {
      expect(screen.getByText(/changed on disk since you started editing/)).toBeTruthy();
    });
    expect(text().value).toBe("timeout_secs = 45\n");
    await fireEvent.click(screen.getByRole("button", { name: "Save config.toml" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("dialog", { name: `${agent}'s config.toml changed` })).toBeTruthy();
    });
    conflictQuestion.answer("use-disk");
    await vi.waitFor(() => {
      expect(toasts()).toContain("Kept config.toml as it is on disk, without your edits.");
    });
    expect(text().value).toBe("timeout_secs = 90\n");
    expect(server.files.config).toBe("timeout_secs = 90\n");
  });
});
