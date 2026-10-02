import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsModel, type AllScopeModel } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import type { A2aKeyInfo, AgentKeyInfo } from "../../lib/types";
import { ConfirmHost } from "../../lib/ui";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  type FetchHandler,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import AgentKeysGroup from "./AgentKeysGroup.svelte";
import CallerKeysGroup from "./CallerKeysGroup.svelte";
import KeysSection from "./KeysSection.svelte";
import ListenerSection from "./ListenerSection.svelte";
import SecretsGroup from "./SecretsGroup.svelte";

// The All agents sections that list what the hub keeps encrypted (Saved
// keys), and the install's agent-to-agent listener with its caller keys. The
// keys act at once through their own endpoints; the listener's switch, port
// and address are staged.

interface Call {
  method: string;
  url: string;
  body: unknown;
}

/** The hub's three key stores over a fake fetch, with every call it saw. */
function keysApi(): {
  handler: FetchHandler;
  calls: Call[];
  agentKeys: AgentKeyInfo[];
  secrets: string[];
  callers: A2aKeyInfo[];
  fail: { list: boolean };
} {
  const state = {
    calls: [] as Call[],
    agentKeys: [
      {
        name: "github_token",
        env_var: "GITHUB_TOKEN",
        description: "Read access to my repos",
        created_by: "user",
      },
      {
        name: "cf_session",
        env_var: "CF_SESSION",
        description: "DNS token the agent minted",
        created_by: "agent",
      },
    ] as AgentKeyInfo[],
    secrets: ["anthropic_key", "openai_key"],
    callers: [
      { name: "laptop", description: "My other install", created_at: "2026-09-26T12:00:00.000Z" },
    ] as A2aKeyInfo[],
    fail: { list: false },
  };
  const removed = new Map<string, AgentKeyInfo | A2aKeyInfo>();
  const handler: FetchHandler = (url, init) => {
    const method = init?.method ?? "GET";
    const body: unknown = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    if (!url.startsWith("/api/hub/")) return new Response("", { status: 599 });
    state.calls.push({ method, url, body });
    const send = (data: unknown): Response => jsonResponse(data);
    if (method === "GET" && state.fail.list && /\/(agent-keys|secrets|a2a\/keys)$/.test(url)) {
      return new Response("", { status: 500 });
    }
    if (url === "/api/hub/agent-keys" && method === "GET") return send({ keys: state.agentKeys });
    if (url === "/api/hub/agent-keys" && method === "POST") {
      const { name, description } = body as { name: string; description: string };
      state.agentKeys.push({
        name,
        env_var: name.toUpperCase(),
        description,
        created_by: "user",
      });
      const short = (body as { value: string }).value.length < 8;
      return send({
        name,
        env_var: name.toUpperCase(),
        ...(short ? { warning: "Too short to redact reliably." } : {}),
      });
    }
    if (method === "DELETE" && url.startsWith("/api/hub/agent-keys/")) {
      const name = decodeURIComponent(url.split("/").pop() ?? "");
      const at = state.agentKeys.findIndex((key) => key.name === name);
      removed.set("cp-keys", state.agentKeys.splice(at, 1)[0] as AgentKeyInfo);
      return send({ deleted: true, checkpoint_id: "cp-keys" });
    }
    if (url === "/api/hub/secrets" && method === "GET") return send({ names: state.secrets });
    if (url === "/api/hub/secrets" && method === "POST") {
      const { name } = body as { name: string };
      if (!state.secrets.includes(name)) state.secrets.push(name);
      return send({ reference: `secret:${name}` });
    }
    if (method === "DELETE" && url.startsWith("/api/hub/secrets/")) {
      const name = decodeURIComponent(url.split("/").pop() ?? "");
      state.secrets.splice(state.secrets.indexOf(name), 1);
      return send({ deleted: true });
    }
    if (url === "/api/hub/a2a/keys" && method === "GET") return send({ keys: state.callers });
    if (url === "/api/hub/a2a/keys" && method === "POST") {
      const { name, description } = body as { name: string; description?: string };
      state.callers.push({
        name,
        description: description ?? "",
        created_at: "2026-09-26T14:00:00.000Z",
      });
      return send({ name, token: `rsdm_a2a_token_for_${name}` });
    }
    if (method === "DELETE" && url.startsWith("/api/hub/a2a/keys/")) {
      const name = decodeURIComponent(url.split("/").pop() ?? "");
      const at = state.callers.findIndex((key) => key.name === name);
      removed.set("cp-callers", state.callers.splice(at, 1)[0] as A2aKeyInfo);
      return send({ revoked: true, checkpoint_id: "cp-callers" });
    }
    const restore = /^\/api\/hub\/checkpoints\/([^/]+)\/restore$/.exec(url);
    if (restore !== null && method === "POST") {
      const checkpoint = restore[1] ?? "";
      const key = removed.get(checkpoint);
      if (checkpoint === "cp-keys" && key !== undefined) state.agentKeys.push(key as AgentKeyInfo);
      if (checkpoint === "cp-callers" && key !== undefined) state.callers.push(key as A2aKeyInfo);
      return send({
        checkpoint_id: "cp-restored",
        restored_paths: [(body as { path: string }).path],
      });
    }
    return new Response("", { status: 599 });
  };
  return Object.assign(state, { handler });
}

let api: ReturnType<typeof keysApi>;
let server: FakeAgentConfig;
/** Answers a request before the fake hub does, or lets it through with `undefined`. */
let override: ((url: string, init: RequestInit | undefined) => Response | undefined) | null = null;
let scope: AllScopeModel;
let count = 0;

/** The hub's config, and the key endpoints answering first. */
async function open(hub = 'timezone = "UTC"\n'): Promise<void> {
  api = keysApi();
  override = null;
  server = fakeAgentConfig(`saved-keys-${String(++count)}`, { hub });
  mockFetch((url, init) => {
    const early = override?.(url, init);
    if (early !== undefined) return early;
    const answer = api.handler(url, init);
    return answer instanceof Response && answer.status === 599 ? server.handler(url, init) : answer;
  });
  scope = settingsModel.all();
  await scope.reload();
}

afterEach(() => {
  scope.discard();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

const messages = (): string[] => [...toast.toasts.values()].map((shown) => shown.message);

/** Press the action of the toast that carries one, as its button does. */
function pressToastAction(): void {
  const [id] = [...toast.toasts.entries()].find(([, shown]) => shown.action !== undefined) ?? [];
  if (id === undefined) throw new Error("no toast has an action");
  toast.runAction(id);
}

/** Wait for every list on screen to finish loading. */
async function loaded(): Promise<void> {
  await settle();
  await vi.waitFor(() => {
    expect(screen.queryAllByText(/^Loading the /)).toHaveLength(0);
  });
}

const callsTo = (method: string, url: string): Call[] =>
  api.calls.filter((call) => call.method === method && call.url === url);

describe("Saved keys", () => {
  beforeEach(async () => {
    await open();
  });

  it("lists agent keys with their variable and who saved them, and secrets by name alone", async () => {
    render(KeysSection, { scope, section: "keys" });
    await loaded();

    const keys = screen.getByRole("list", { name: "Agent keys" });
    expect(keys).toHaveTextContent("github_token");
    expect(keys).toHaveTextContent("$GITHUB_TOKEN");
    expect(keys).toHaveTextContent("Read access to my repos");
    expect(keys).toHaveTextContent("$CF_SESSION");
    expect(keys).toHaveTextContent("Saved by the agent");
    const secrets = screen.getByRole("list", { name: "Stored secrets" });
    expect(secrets).toHaveTextContent("anthropic_key");
    expect(secrets).toHaveTextContent("openai_key");
    expect(scope.dirty).toBe(false);
  });

  it("says each list is empty, with what to do next", async () => {
    render(KeysSection, { scope, section: "keys" });
    await loaded();
    for (const key of [...api.agentKeys]) {
      await fireEvent.click(screen.getByRole("button", { name: `Remove ${key.name}` }));
      await vi.waitFor(() => {
        expect(screen.queryByRole("button", { name: `Remove ${key.name}` })).toBeNull();
      });
    }

    expect(screen.getByText(/No keys yet\. Add one here/)).toBeInTheDocument();
  });

  it("adds an agent key with the variable it will have, and tells the user", async () => {
    const user = userEvent.setup();
    render(AgentKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a key" }));

    expect(screen.getByLabelText("Name")).toHaveFocus();
    await user.type(screen.getByLabelText("Name"), "stripe_live");
    expect(screen.getByText("Commands that use it get $STRIPE_LIVE.")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Value"), "sk_live_abcdefghij");
    await user.type(screen.getByLabelText("Description"), "Billing");
    await user.click(screen.getByRole("button", { name: "Save key" }));
    await settle();

    expect(callsTo("POST", "/api/hub/agent-keys")[0]?.body).toEqual({
      name: "stripe_live",
      value: "sk_live_abcdefghij",
      description: "Billing",
    });
    expect(messages()).toContain("Saved stripe_live. Commands that use it get $STRIPE_LIVE.");
    expect(screen.getByRole("list", { name: "Agent keys" })).toHaveTextContent("stripe_live");
    expect(screen.queryByRole("form", { name: "Add an agent key" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a key" })).toHaveFocus();
  });

  it("holds Save key until the name is one the hub accepts, and says what a name looks like", async () => {
    const user = userEvent.setup();
    render(AgentKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a key" }));
    await user.type(screen.getByLabelText("Value"), "a-long-enough-value");

    await user.type(screen.getByLabelText("Name"), "GitHub Token");
    expect(screen.getByText(/Start with a lowercase letter/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save key" })).toBeDisabled();

    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "github_token");
    expect(screen.getByText(/Saving replaces the existing key\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save key" })).toBeEnabled();
  });

  it("hints that a short value can't be hidden reliably, and still saves it with the hub's warning", async () => {
    const user = userEvent.setup();
    render(AgentKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a key" }));
    await user.type(screen.getByLabelText("Name"), "pin");
    await user.type(screen.getByLabelText("Value"), "1234");
    expect(screen.getByText(/under 8 characters can't be hidden/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Save key" }));
    await settle();

    expect(messages()).toContain("Too short to redact reliably.");
    expect(api.agentKeys.map((key) => key.name)).toContain("pin");
  });

  it("never puts a typed value back on screen once the key is saved", async () => {
    const user = userEvent.setup();
    render(AgentKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a key" }));
    await user.type(screen.getByLabelText("Name"), "stripe_live");
    await user.type(screen.getByLabelText("Value"), "sk_live_abcdefghij");
    await user.click(screen.getByRole("button", { name: "Save key" }));
    await settle();

    expect(document.body).not.toHaveTextContent("sk_live_abcdefghij");
  });

  it("removes an agent key at once and offers Undo from the checkpoint the hub took", async () => {
    render(AgentKeysGroup);
    await loaded();

    await fireEvent.click(screen.getByRole("button", { name: "Remove github_token" }));
    await vi.waitFor(() => {
      expect(screen.queryByText("github_token")).not.toBeInTheDocument();
    });

    expect(messages()).toContain("Removed github_token.");
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Add a key" })).toHaveFocus();
    });
    pressToastAction();
    await vi.waitFor(() => {
      expect(screen.getByRole("list", { name: "Agent keys" })).toHaveTextContent("github_token");
    });
    expect(callsTo("POST", "/api/hub/checkpoints/cp-keys/restore")[0]?.body).toEqual({
      repo: "hub",
      path: "agent-keys.toml.enc",
    });
    expect(messages()).toContain("Restored.");
  });

  it("names what failed when a key can't be removed, and keeps the list", async () => {
    render(AgentKeysGroup);
    await loaded();
    override = (_url, init) =>
      init?.method === "DELETE" ? new Response("", { status: 500 }) : undefined;

    await fireEvent.click(screen.getByRole("button", { name: "Remove github_token" }));
    await vi.waitFor(() => {
      expect(
        messages().some((message) => message.startsWith("Couldn't remove github_token.")),
      ).toBe(true);
    });

    expect(messages().some((message) => message.startsWith("Couldn't remove github_token."))).toBe(
      true,
    );
    expect(screen.getByRole("list", { name: "Agent keys" })).toHaveTextContent("github_token");
  });

  it("adds a secret, warning first when the name is already stored", async () => {
    const user = userEvent.setup();
    render(SecretsGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a secret" }));

    await user.type(screen.getByLabelText("Name"), "openai_key");
    expect(screen.getByText("Saving replaces the stored secret with this name.")).toBeVisible();
    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "discord");
    await user.type(screen.getByLabelText("Value"), "xoxb-123");
    await user.click(screen.getByRole("button", { name: "Save secret" }));
    await settle();

    expect(callsTo("POST", "/api/hub/secrets")[0]?.body).toEqual({
      name: "discord",
      value: "xoxb-123",
    });
    expect(messages()).toContain("Saved discord.");
    expect(screen.getByRole("list", { name: "Stored secrets" })).toHaveTextContent("discord");
    expect(document.body).not.toHaveTextContent("xoxb-123");
  });

  it("asks before removing a secret, and removes nothing when the user declines", async () => {
    const user = userEvent.setup();
    render(ConfirmHost);
    render(SecretsGroup);
    await loaded();

    await user.click(screen.getByRole("button", { name: "Remove openai_key" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove openai_key?" });
    expect(dialog).toHaveTextContent("can't be recovered from here");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await settle();

    expect(callsTo("DELETE", "/api/hub/secrets/openai_key")).toEqual([]);
    expect(screen.getByRole("list", { name: "Stored secrets" })).toHaveTextContent("openai_key");
  });

  it("removes a secret once the user confirms", async () => {
    const user = userEvent.setup();
    render(ConfirmHost);
    render(SecretsGroup);
    await loaded();

    await user.click(screen.getByRole("button", { name: "Remove openai_key" }));
    await user.click(await screen.findByRole("button", { name: "Remove secret" }));
    await settle();

    expect(callsTo("DELETE", "/api/hub/secrets/openai_key")).toHaveLength(1);
    expect(messages()).toContain("Removed openai_key.");
    expect(screen.queryByText("openai_key")).not.toBeInTheDocument();
  });

  it("shows a list that can't load as an error with Try again, and the other list still loads", async () => {
    api.fail.list = true;
    render(KeysSection, { scope, section: "keys" });
    await loaded();
    expect(screen.getAllByRole("alert")).toHaveLength(2);
    expect(screen.getByText(/Couldn't load the agent keys\./)).toBeInTheDocument();
    expect(screen.getByText(/Couldn't load the stored secrets\./)).toBeInTheDocument();

    api.fail.list = false;
    await fireEvent.click(screen.getAllByRole("button", { name: "Try again" })[0] as HTMLElement);
    await settle();

    expect(screen.getByRole("list", { name: "Agent keys" })).toBeInTheDocument();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });
});

describe("Agent-to-agent listener", () => {
  beforeEach(async () => {
    await open('timezone = "UTC"\n\n[a2a]\nenabled = true\nport = 7702\n');
  });

  it("stages the switch, the port and the address with the rest of the install's settings", async () => {
    const user = userEvent.setup();
    render(ListenerSection, { scope, section: "listener" });
    await loaded();
    expect(screen.getByLabelText("Listener port")).toHaveValue(7702);

    await user.clear(screen.getByLabelText("Listener port"));
    await user.type(screen.getByLabelText("Listener port"), "7800");
    await user.type(screen.getByLabelText("Your own address"), "https://tunnel.example/a2a");

    expect(scope.configFile.patch).toEqual({
      a2a: { port: 7800, public_url: "https://tunnel.example/a2a" },
    });
    expect(callsTo("PATCH", "/api/hub/config/patch")).toEqual([]);
  });

  it("dims the port and address while the listener is off, instead of hiding them", async () => {
    render(ListenerSection, { scope, section: "listener" });
    await loaded();
    expect(screen.getByLabelText("Listener port")).toBeEnabled();

    await fireEvent.click(
      screen.getByRole("switch", { name: "Let other agents reach this install" }),
    );

    expect(screen.getByLabelText("Listener port")).toBeDisabled();
    expect(screen.getByLabelText("Your own address")).toBeDisabled();
    expect(scope.configFile.patch).toEqual({ a2a: { enabled: false } });
  });

  it("shows a problem the server found on the address on the field", async () => {
    const answer = jsonResponse({
      valid: false,
      diagnostics: [
        {
          severity: "error",
          message: "a2a.public_url must start with https://",
          location: { kind: "path", path: "a2a.public_url" },
        },
      ],
    });
    const user = userEvent.setup();
    render(ListenerSection, { scope, section: "listener" });
    await loaded();
    await user.type(screen.getByLabelText("Your own address"), "tunnel.example");
    override = (url, init) =>
      init?.method === "PATCH" && url === "/api/hub/config/patch" ? answer : undefined;

    await scope.save(() => Promise.resolve("keep-mine"));
    await settle();

    expect(screen.getByText("a2a.public_url must start with https://")).toBeInTheDocument();
  });
});

describe("Caller keys", () => {
  beforeEach(async () => {
    await open();
  });

  it("lists the keys with what they are for and when they were made", async () => {
    vi.useFakeTimers({ now: new Date("2026-09-26T15:00:00Z") });
    render(CallerKeysGroup);
    await loaded();

    const list = screen.getByRole("list", { name: "Caller keys" });
    expect(list).toHaveTextContent("laptop");
    expect(list).toHaveTextContent("My other install");
    expect(list).toHaveTextContent("Created 3h ago");
  });

  it("creates a key and shows its token, which Copy puts on the clipboard", async () => {
    const user = userEvent.setup();
    render(CallerKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a caller key" }));
    await user.type(screen.getByLabelText("Name"), "phone");
    await user.type(screen.getByLabelText("Description"), "My phone");
    await user.click(screen.getByRole("button", { name: "Create key" }));
    await settle();

    expect(callsTo("POST", "/api/hub/a2a/keys")[0]?.body).toEqual({
      name: "phone",
      description: "My phone",
    });
    const reveal = screen.getByText("Key for phone created.").closest("[role=status]");
    expect(reveal).toHaveTextContent("rsdm_a2a_token_for_phone");
    expect(reveal).toHaveTextContent("you won't see it again");
    expect(screen.getByRole("button", { name: "Copy key" })).toHaveFocus();
    expect(screen.getByRole("list", { name: "Caller keys" })).toHaveTextContent("phone");

    await user.click(screen.getByRole("button", { name: "Copy key" }));
    expect(await navigator.clipboard.readText()).toBe("rsdm_a2a_token_for_phone");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("drops the token for good once the user is done with it", async () => {
    const user = userEvent.setup();
    render(CallerKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a caller key" }));
    await user.type(screen.getByLabelText("Name"), "phone");
    await user.click(screen.getByRole("button", { name: "Create key" }));
    await settle();

    await user.click(screen.getByRole("button", { name: "Done" }));

    expect(document.body).not.toHaveTextContent("rsdm_a2a_token_for_phone");
    expect(screen.getByRole("button", { name: "Add a caller key" })).toHaveFocus();
  });

  it("says so when the token can't be copied, since it is only here once", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    render(CallerKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a caller key" }));
    await user.type(screen.getByLabelText("Name"), "phone");
    await user.click(screen.getByRole("button", { name: "Create key" }));
    await settle();

    await user.click(screen.getByRole("button", { name: "Copy key" }));

    expect(messages()).toContain("Couldn't copy the key. Select it and copy it by hand.");
    expect(document.body).toHaveTextContent("rsdm_a2a_token_for_phone");
  });

  it("holds Create key for a name that exists or that the hub would refuse", async () => {
    const user = userEvent.setup();
    render(CallerKeysGroup);
    await loaded();
    await user.click(screen.getByRole("button", { name: "Add a caller key" }));

    await user.type(screen.getByLabelText("Name"), "laptop");
    expect(screen.getByText("A key named laptop already exists.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create key" })).toBeDisabled();

    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "9lives");
    expect(screen.getByText(/Start with a lowercase letter/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create key" })).toBeDisabled();
  });

  it("revokes at once and offers Undo from the checkpoint the hub took", async () => {
    render(CallerKeysGroup);
    await loaded();

    await fireEvent.click(screen.getByRole("button", { name: "Revoke laptop" }));
    await screen.findByText(/No caller keys yet/);

    expect(messages()).toContain("Revoked laptop. It can no longer reach your agents.");
    pressToastAction();
    await vi.waitFor(() => {
      expect(screen.getByRole("list", { name: "Caller keys" })).toHaveTextContent("laptop");
    });
    expect(callsTo("POST", "/api/hub/checkpoints/cp-callers/restore")[0]?.body).toEqual({
      repo: "hub",
      path: "a2a-keys.toml",
    });
  });
});
