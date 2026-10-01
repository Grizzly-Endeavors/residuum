import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CACHE_KEY_MCP_CATALOG } from "../../lib/api";
import { invalidate } from "../../lib/cache";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import type { McpCatalogEntry } from "../../lib/types";
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

const CATALOG: McpCatalogEntry[] = [
  {
    name: "github",
    description: "GitHub repos, issues and pull requests",
    command: "npx",
    args: ["-y", "@org/server-github"],
    env: { GITHUB_TOKEN: "" },
    category: "dev",
    requires_input: [{ field: "env.GITHUB_TOKEN", label: "GitHub token" }],
    install_hint: "Requires Node.js 18+",
  },
  {
    name: "fetch",
    description: "Fetch web pages",
    command: "npx",
    args: ["-y", "@org/server-fetch"],
    env: {},
    category: "web",
    requires_input: [],
    install_hint: "",
  },
];

let agent = "";
let server: FakeAgentConfig;
let count = 0;
let catalogFails = 0;

const saveBar = (): HTMLElement | null => screen.queryByRole("region", { name: "Unsaved changes" });
const mcpPatches = (): unknown[] =>
  server.requests
    .filter((request) => request.method === "PATCH" && request.url.endsWith("/mcp/patch"))
    .map((request) => JSON.parse(request.body ?? "{}") as unknown);
const serverNames = (): string[] =>
  [...screen.getByRole("list", { name: "Servers" }).querySelectorAll(".server-name")].map(
    (name) => name.firstChild?.textContent?.trim() ?? "",
  );

async function open(): Promise<void> {
  render(SettingsModal);
  await router.openSettings({ scope: agent, section: "servers" });
  await settle();
  await vi.waitFor(() => {
    expect(screen.getByRole("heading", { name: "Tool servers" })).toBeTruthy();
  });
}

async function type(input: HTMLElement, value: string): Promise<void> {
  await fireEvent.input(input, { target: { value } });
  await settle();
}

async function press(name: string): Promise<void> {
  await fireEvent.click(screen.getByRole("button", { name }));
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
  invalidate(CACHE_KEY_MCP_CATALOG);
  catalogFails = 0;
  agent = `servers-${String(++count)}`;
  server = fakeAgentConfig(agent, {
    mcp: JSON.stringify({ mcpServers: { files: { command: "fs-server", args: ["/data"] } } }),
  });
  mockFetch((url, init) => {
    if (url === "/api/hub/mcp-catalog") {
      if (catalogFails > 0) {
        catalogFails -= 1;
        return jsonResponse({ error: "catalog unavailable" }, 503);
      }
      return jsonResponse(CATALOG);
    }
    return server.handler(url, init);
  });
});

afterEach(async () => {
  await router.replacePlace({ kind: "home" });
  for (const scope of settingsModel.stagedScopes) scope.discard();
});

describe("the Tool servers section", () => {
  it("shows a catalog that couldn't be read as an error with Try again, never as empty", async () => {
    catalogFails = 1;
    await open();
    await vi.waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("Couldn't read the tool server catalog.");
    });
    expect(screen.queryByText("The catalog has no servers in it.")).toBeNull();
    await press("Try again");
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Add fetch" })).toBeTruthy();
    });
  });

  it("asks for a catalog server's key, then stages it with the key in its variables", async () => {
    await open();
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Add github" })).toBeTruthy();
    });
    await press("Add github");
    await fireEvent.submit(screen.getByRole("form", { name: "Add github" }));
    await settle();
    expect(screen.getByText("Enter the GitHub token.")).toBeTruthy();
    expect(saveBar()).toBeNull();

    await type(screen.getByLabelText("GitHub token"), " ghp_123 ");
    await fireEvent.submit(screen.getByRole("form", { name: "Add github" }));
    await settle();
    expect(serverNames()).toEqual(["files", "github"]);
    expect(screen.getByText("Added")).toBeTruthy();
    expect(saveBar()).toHaveTextContent("You have unsaved changes.");

    await press("Save changes");
    await vi.waitFor(() => {
      expect(saveBar()).toBeNull();
    });
    expect(mcpPatches()).toEqual([
      {
        mcpServers: {
          github: {
            type: "stdio",
            command: "npx",
            args: ["-y", "@org/server-github"],
            env: { GITHUB_TOKEN: "ghp_123" },
          },
        },
      },
    ]);
  });

  it("stages a removal, which Discard brings back without writing anything", async () => {
    await open();
    await press("Remove files");
    expect(screen.queryByRole("list", { name: "Servers" })).toBeNull();
    expect(saveBar()).toHaveTextContent("You have unsaved changes.");
    await press("Discard");
    expect(serverNames()).toEqual(["files"]);
    expect(mcpPatches()).toEqual([]);
  });

  it("edits a server's arguments one per line, and saves only what changed", async () => {
    await open();
    await press("Edit files");
    expect(screen.getByLabelText("Arguments")).toHaveValue("/data");
    await type(screen.getByLabelText("Arguments"), "/data\n/home/bear/My Notes\n");
    await type(screen.getByLabelText("Environment variables"), "LOG=debug\nnot a pair");
    expect(screen.getByText(/Line 2 has no NAME= before the value/)).toBeTruthy();
    await press("Save changes");
    await vi.waitFor(() => {
      expect(mcpPatches()).toEqual([
        {
          mcpServers: {
            files: { args: ["/data", "/home/bear/My Notes"], env: { LOG: "debug" } },
          },
        },
      ]);
    });
  });

  it("adds a server by hand, checking its name and what it needs first", async () => {
    await open();
    await press("Add a server");
    expect(document.activeElement).toBe(screen.getByLabelText("Name"));
    await fireEvent.submit(screen.getByRole("form", { name: "Add a tool server" }));
    await settle();
    expect(screen.getByText("Give the server a name.")).toBeTruthy();
    expect(screen.getByText("Enter the command that starts it.")).toBeTruthy();

    await type(screen.getByLabelText("Name"), "files");
    expect(screen.getByText("There's already a server named files.")).toBeTruthy();
    await type(screen.getByLabelText("Name"), "search");
    await fireEvent.click(screen.getByRole("radio", { name: "Web address" }));
    await settle();
    await type(screen.getByLabelText("Address"), "https://search.example/mcp");
    await type(screen.getByLabelText("Headers"), "Authorization=Bearer ${agent-key:search}");
    await fireEvent.submit(screen.getByRole("form", { name: "Add a tool server" }));
    await settle();

    expect(serverNames()).toEqual(["files", "search"]);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add a server" }));
    await press("Save changes");
    await vi.waitFor(() => {
      expect(mcpPatches()).toEqual([
        {
          mcpServers: {
            search: {
              type: "http",
              url: "https://search.example/mcp",
              headers: { Authorization: "Bearer ${agent-key:search}" },
            },
          },
        },
      ]);
    });
  });
});
