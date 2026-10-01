import { describe, expect, it, vi } from "vitest";
import { matchActions, type AppAction } from "./action-registry.svelte";
import { chatActions, connectionStatusMessage, type ChatActionContext } from "./chat-actions";

function context(overrides: Partial<ChatActionContext> = {}): ChatActionContext {
  return {
    agent: "atlas",
    state: "running",
    replying: false,
    hubConnection: "connected",
    agentConnection: "connected",
    send: vi.fn(),
    stopReply: vi.fn(),
    surface: vi.fn(),
    showConversationSize: vi.fn(),
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
    // One message each: Summarize and Condense say they started, and the
    // agent's own notice says when they're done, or that it reloaded or added the note.
    expect(vi.mocked(ctx.surface).mock.calls).toEqual([
      ["system", "atlas is summarizing older messages…"],
      ["system", "atlas is condensing its memories…"],
      ["system", "Stopping atlas's reply."],
    ]);
  });

  it("leave /verbose out: tool activity always shows", () => {
    expect(matchActions(chatActions(context()), "/verbose")).toEqual([]);
  });

  it("show the conversation size in the panel, even for an agent that isn't running", () => {
    const ctx = context({ state: "stopped" });
    const size = byId(chatActions(ctx), "chat:context");
    expect(size.disabled).toBeUndefined();
    size.run();
    expect(ctx.showConversationSize).toHaveBeenCalledWith("atlas");
    expect(ctx.send).not.toHaveBeenCalled();
  });

  it("ask for a note's text when the inbox action has none", () => {
    const ctx = context();
    byId(chatActions(ctx), "chat:inbox").run("  ");
    expect(ctx.askForInboxNote).toHaveBeenCalledWith("atlas");
    expect(ctx.send).not.toHaveBeenCalled();
  });
});

describe("disabled reasons", () => {
  const reasons = (ctx: ChatActionContext): Record<string, string | undefined> =>
    Object.fromEntries(chatActions(ctx).map((a) => [a.id, a.disabled]));

  it("ask for the agent to be started when it isn't running", () => {
    const disabled = reasons(context({ state: "stopped" }));
    for (const id of ["chat:observe", "chat:reflect", "chat:reload", "chat:inbox"]) {
      expect(disabled[id], id).toBe("Start atlas first");
    }
    expect(disabled["chat:stop"]).toBe("Start atlas first");
    expect(reasons(context({ state: "failed" }))["chat:observe"]).toBe("Start atlas first");
  });

  it("say when the agent is on its way up or down", () => {
    expect(reasons(context({ state: "starting" }))["chat:observe"]).toBe("atlas is still starting");
    expect(reasons(context({ state: "stopping" }))["chat:observe"]).toBe("atlas is stopping");
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
