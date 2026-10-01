import { describe, expect, it, vi } from "vitest";
import { matchActions, type AppAction } from "./action-registry.svelte";
import { chatActions, connectionStatusMessage, type ChatActionContext } from "./chat-actions";

function context(overrides: Partial<ChatActionContext> = {}): ChatActionContext {
  return {
    agent: "atlas",
    state: "running",
    stopping: false,
    replying: false,
    verbose: false,
    hubConnection: "connected",
    agentConnection: "connected",
    send: vi.fn(),
    stopReply: vi.fn(),
    setVerbose: vi.fn(),
    surface: vi.fn(),
    openChat: vi.fn(),
    askForInboxNote: vi.fn(),
    ...overrides,
  };
}

function byId(actions: readonly AppAction[], id: string): AppAction {
  const found = actions.find((a) => a.id === id);
  if (found === undefined) throw new Error(`no action ${id}`);
  return found;
}

describe("the former slash commands", () => {
  it.each([
    ["/observe", "Summarize older messages now"],
    ["/reflect", "Condense memories now"],
    ["/context", "Show conversation size"],
    ["/reload", "Reload settings"],
    ["/stop", "Stop reply"],
    ["/inbox", "Add a note to atlas's inbox"],
    ["/verbose", "Show tool calls"],
    ["/status", "Show connection status"],
  ])("are found by their old name: %s is %s", (command, label) => {
    expect(matchActions(chatActions(context()), command).map((a) => a.label)).toEqual([label]);
  });

  it("send what the slash commands sent", () => {
    const ctx = context({ replying: true });
    const actions = chatActions(ctx);
    byId(actions, "chat:observe").run();
    byId(actions, "chat:reflect").run();
    byId(actions, "chat:reload").run();
    byId(actions, "chat:inbox").run("  water the plants ");
    byId(actions, "chat:stop").run();
    expect(vi.mocked(ctx.send).mock.calls.map(([msg]) => msg)).toEqual([
      { type: "server_command", name: "observe", args: null },
      { type: "server_command", name: "reflect", args: null },
      { type: "reload" },
      { type: "inbox_add", body: "water the plants" },
    ]);
    expect(ctx.stopReply).toHaveBeenCalledOnce();
    expect(ctx.surface).toHaveBeenCalledWith("notice", "Added a note to atlas's inbox.");
  });

  it("show the conversation size in the agent's chat, where the answer lands", () => {
    const ctx = context();
    byId(chatActions(ctx), "chat:context").run();
    expect(ctx.openChat).toHaveBeenCalledWith("atlas");
    expect(ctx.send).toHaveBeenCalledWith({ type: "server_command", name: "context", args: null });
  });

  it("ask for a note's text when the inbox action has none", () => {
    const ctx = context();
    byId(chatActions(ctx), "chat:inbox").run("  ");
    expect(ctx.askForInboxNote).toHaveBeenCalledWith("atlas");
    expect(ctx.send).not.toHaveBeenCalled();
  });

  it("flip tool calls and say which way", () => {
    const ctx = context({ verbose: true });
    const verbose = byId(chatActions(ctx), "chat:verbose");
    expect(verbose.label).toBe("Hide tool calls");
    verbose.run();
    expect(ctx.setVerbose).toHaveBeenCalledWith(false);
    expect(ctx.surface).toHaveBeenCalledWith("system", "Tool calls are hidden.");
  });
});

describe("disabled reasons", () => {
  const reasons = (ctx: ChatActionContext): Record<string, string | undefined> =>
    Object.fromEntries(chatActions(ctx).map((a) => [a.id, a.disabled]));

  it("ask for the agent to be started when it isn't running", () => {
    const disabled = reasons(context({ state: "stopped" }));
    for (const id of [
      "chat:observe",
      "chat:reflect",
      "chat:context",
      "chat:reload",
      "chat:inbox",
    ]) {
      expect(disabled[id], id).toBe("Start atlas first");
    }
    expect(disabled["chat:stop"]).toBe("Start atlas first");
    expect(reasons(context({ state: "failed" }))["chat:observe"]).toBe("Start atlas first");
  });

  it("say when the agent is on its way up or down", () => {
    expect(reasons(context({ state: "starting" }))["chat:observe"]).toBe("atlas is still starting");
    expect(reasons(context({ stopping: true }))["chat:observe"]).toBe("atlas is stopping");
    expect(reasons(context({ agent: null, state: null }))["chat:observe"]).toBe(
      "Open an agent first",
    );
  });

  it("leave Stop reply off while nothing is replying", () => {
    expect(reasons(context())["chat:stop"]).toBe("atlas isn't replying right now");
    expect(reasons(context({ replying: true }))["chat:stop"]).toBeUndefined();
  });

  it("never hold back the actions that don't need the agent", () => {
    const disabled = reasons(context({ state: "stopped" }));
    expect(disabled["chat:verbose"]).toBeUndefined();
    expect(disabled["chat:status"]).toBeUndefined();
  });
});

describe("connectionStatusMessage", () => {
  it("names the hub and the agent connection in plain words", () => {
    expect(connectionStatusMessage(context())).toBe("Connected to Residuum. atlas is connected.");
    expect(connectionStatusMessage(context({ agentConnection: "connecting" }))).toBe(
      "Connected to Residuum. Reconnecting to atlas. Messages you send will go out once it's back.",
    );
    expect(
      connectionStatusMessage(context({ hubConnection: "disconnected", state: "stopped" })),
    ).toBe(
      "Can't reach Residuum right now. Trying again. atlas isn't running, so there's no connection to it.",
    );
  });
});
