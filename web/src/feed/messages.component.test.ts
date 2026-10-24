import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen, settle } from "../test/component";
import { notifications } from "../lib/notifications.svelte";
import { FeedStore } from "../lib/feed.svelte";
import type { FeedItem, UndoOutcome } from "../lib/types";
import type * as FeedLinks from "./feed-links";
import FeedItemView from "./FeedItemView.svelte";

const { turnChangedWorkspace, undoTurn, openSessionByAddress, openAgentFileInPanel } = vi.hoisted(
  () => ({
    turnChangedWorkspace: vi.fn<(agent: string | null, turnId: string) => Promise<boolean>>(),
    undoTurn: vi.fn<(agent: string | null, turnId: string) => Promise<UndoOutcome | null>>(),
    openSessionByAddress:
      vi.fn<(agent: string, address: string, runId: string | null) => Promise<void>>(),
    openAgentFileInPanel: vi.fn<(agent: string, path: string) => void>(),
  }),
);
vi.mock("../lib/turn-undo", () => ({ turnChangedWorkspace, undoTurn }));
vi.mock("../lib/session-address", () => ({ openSessionByAddress }));
vi.mock("./feed-links", async (importOriginal) => ({
  ...(await importOriginal<typeof FeedLinks>()),
  openAgentFileInPanel,
}));

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** Show `item` the way the feed holds it: inside the store's reactive feed. */
function show(item: FeedItem, agent = "atlas"): void {
  const store = new FeedStore();
  store.feed.push(item);
  const held = store.feed[0];
  if (held === undefined) throw new Error("the feed didn't take the item");
  render(FeedItemView, { item: held, agent });
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", NoObserver);
  openAgentFileInPanel.mockReset();
  turnChangedWorkspace.mockReset();
  undoTurn.mockReset();
  openSessionByAddress.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("the user's message", () => {
  it("shows the text, with no sender line for the user's own message", () => {
    show({ id: 1, kind: "user", content: "Keep an eye on the release." });
    expect(screen.getByText("Keep an eye on the release.")).toBeInTheDocument();
    expect(screen.queryByText(/·/)).toBeNull();
  });

  it("names a sender from another interface", () => {
    show({
      id: 1,
      kind: "user",
      content: "did the build go out?",
      sender: { name: "maya", id: "u1", interface: "discord", location: "#builds" },
    });
    expect(screen.getByText("maya · discord · #builds")).toBeInTheDocument();
  });

  it("shows attached images inline", () => {
    show({
      id: 1,
      kind: "user",
      content: "",
      images: [
        { media_type: "image/png", data: "AAAA" },
        { media_type: "image/jpeg", data: "BBBB" },
      ],
    });
    expect(screen.getByRole("img", { name: "Attached image 2" })).toHaveAttribute(
      "src",
      "data:image/jpeg;base64,BBBB",
    );
  });

  it("offers Undo this turn on the agent it belongs to, once the turn changed files", async () => {
    turnChangedWorkspace.mockResolvedValue(true);
    undoTurn.mockResolvedValue({
      checkpoint_id: "cp-undo",
      reverted_paths: ["a.md", "b.md"],
      skipped_paths: ["c.md"],
    });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    show(
      { id: 1, kind: "user", content: "tidy up", turn: { turnId: "t-1", changed: null } },
      "scout",
    );
    await settle();

    expect(turnChangedWorkspace).toHaveBeenCalledWith("scout", "t-1");
    await userEvent.click(screen.getByRole("button", { name: "Undo this turn" }));
    await settle();

    expect(undoTurn).toHaveBeenCalledWith("scout", "t-1");
    expect(surface).toHaveBeenCalledWith(
      "notice",
      "Undid this turn: put back 2 files. Left c.md alone: it was changed again after this turn.",
    );
    expect(screen.queryByRole("button", { name: "Undo this turn" })).toBeNull();
  });

  it("offers no undo for a turn that changed nothing", async () => {
    turnChangedWorkspace.mockResolvedValue(false);
    show({ id: 1, kind: "user", content: "hello", turn: { turnId: "t-2", changed: null } });
    await settle();
    expect(screen.queryByRole("button", { name: "Undo this turn" })).toBeNull();
  });
});

describe("the files a user's message carried", () => {
  const notes = {
    item_id: "item-1",
    filename: "notes.md",
    size: 1840,
    content_type: "text/markdown",
    path: "inbox/agent/attachments/item-1/notes.md",
  };
  const spec = {
    item_id: "item-2",
    filename: "spec.pdf",
    size: 48213,
    content_type: "application/pdf",
    path: "inbox/agent/attachments/item-2/spec.pdf",
  };

  it("shows a chip for each file, a download link for a non-text file", () => {
    show({ id: 1, kind: "user", content: "", attachments: [notes, spec] }, "atlas");
    const list = screen.getByRole("list", { name: "Attachments" });
    expect(list.querySelectorAll("li")).toHaveLength(2);
    expect(screen.getByRole("link", { name: "Download spec.pdf" })).toHaveAttribute(
      "href",
      "/api/agents/atlas/inbox/agent/item-2/attachments/0",
    );
  });

  it("opens a Markdown file in the context panel beside the agent", async () => {
    show({ id: 1, kind: "user", content: "", attachments: [notes] }, "atlas");
    expect(screen.queryByRole("link", { name: "Download notes.md" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Open notes.md" }));
    expect(openAgentFileInPanel).toHaveBeenCalledWith("atlas", notes.path);
  });

  it("shows a chip with no link for a file whose item isn't saved yet", () => {
    show({
      id: 1,
      kind: "user",
      content: "",
      attachments: [{ ...spec, item_id: "" }],
    });
    expect(screen.getByText("spec")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open spec.pdf" })).toBeNull();
  });

  it("hides the attachment lines and keeps the rest of the text", () => {
    show({
      id: 1,
      kind: "user",
      content: [
        "Here is the spec.",
        `[Attachment: notes.md (1840 bytes) → ${notes.path}]`,
        `[Attachment: spec.pdf (48213 bytes) → ${spec.path}]`,
        "Tell me what you think.",
      ].join("\n"),
      attachments: [notes, spec],
    });
    expect(screen.getByText(/Here is the spec\./)).toBeInTheDocument();
    expect(screen.getByText(/Tell me what you think\./)).toBeInTheDocument();
    expect(screen.queryByText(/\[Attachment:/)).toBeNull();
  });

  it("keeps a download-failed line, which no chip stands for", () => {
    const failed = "[Attachment: spec.pdf (48213 bytes) — download failed: timeout]";
    show({ id: 1, kind: "user", content: failed, attachments: [] });
    expect(screen.getByText(failed)).toBeInTheDocument();
  });
});

describe("the agent's reply", () => {
  it("is Markdown in the reading column", () => {
    show({ id: 1, kind: "assistant", content: "Two **changes** touch us." });
    expect(screen.getByText("changes").tagName).toBe("STRONG");
  });
});

describe("a message from a session or a teammate", () => {
  it("names a session's kind and address, and opens the session on the feed's agent", async () => {
    show(
      {
        id: 1,
        kind: "agent-message",
        from: "spawned-research-3f9a",
        category: "spawned",
        content: "Found three fallback strategies.",
        runId: "run-9",
      },
      "scout",
    );
    const card = screen.getByRole("article", {
      name: "Background session: spawned-research-3f9a",
    });
    expect(card).toHaveTextContent("Found three fallback strategies.");

    await userEvent.click(screen.getByRole("button", { name: "Open session" }));

    expect(openSessionByAddress).toHaveBeenCalledWith("scout", "spawned-research-3f9a", "run-9");
  });

  it("offers no session to open for a teammate", () => {
    show({
      id: 1,
      kind: "agent-message",
      from: "agent:scout",
      category: "teammate",
      content: "Can you look over the wiki index?",
      runId: null,
    });
    expect(screen.getByRole("article", { name: "Teammate: scout" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open session" })).toBeNull();
  });

  it("shows a teammate's live message as the card history gives it, without its header", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "user_message",
      id: "bg-1",
      turn_id: "bg-1",
      content:
        "[Message from teammate agent:scout, not the user. Your response in this turn is not " +
        'shown to them; to reply, call message_agent with to="agent:scout".]\n' +
        "Can you look over the wiki index?",
      agent_sender: { address: "agent:scout", category: "teammate" },
      endpoint: "background",
    });
    const held = store.feed[0];
    if (held === undefined) throw new Error("the frame added nothing to the feed");
    render(FeedItemView, { item: held, agent: "atlas" });

    const card = screen.getByRole("article", { name: "Teammate: scout" });
    expect(card).toHaveTextContent("Can you look over the wiki index?");
    expect(card).not.toHaveTextContent("Message from teammate");
    expect(screen.queryByRole("button", { name: "Open session" })).toBeNull();
  });
});

describe("dividers and markers", () => {
  it("names a day from today, and an episode by its day with the id kept quiet", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 8, 12, 0));
    show({
      id: 1,
      kind: "divider",
      variant: "episode",
      label: "",
      date: "2026-10-07",
      episode: "ep-003",
    });
    const divider = screen.getByRole("separator", { name: "Yesterday, ep-003" });
    expect(divider).toHaveTextContent(/^Yesterday\s*ep-003$/);
  });

  it("shows a divider that stands for no day as it is", () => {
    show({ id: 1, kind: "divider", variant: "day", label: "New run" });
    expect(screen.getByRole("separator", { name: "New run" })).toHaveTextContent("New run");
  });

  it("explains the compressed history in plain words, naming the agent", () => {
    show({ id: 1, kind: "compressed-marker" }, "scout");
    expect(screen.getByRole("note")).toHaveTextContent(
      "Older messages are summarized. scout remembers what was said, not the exact wording.",
    );
  });
});

describe("a file the agent sent", () => {
  it("shows an image inline with its caption, and the file to download", () => {
    show({
      id: 1,
      kind: "file-attachment",
      filename: "chart.png",
      mimeType: "image/png",
      size: 20480,
      url: "/files/chart.png",
      caption: "This week's usage",
    });
    expect(screen.getByRole("img", { name: "This week's usage" })).toHaveAttribute(
      "src",
      "/files/chart.png",
    );
    const download = screen.getByRole("link", { name: /chart\.png/ });
    expect(download).toHaveAttribute("download", "chart.png");
    expect(download).toHaveTextContent("20.0 KB");
  });

  it("plays audio inline", () => {
    const { container } = render(FeedItemView, {
      item: {
        id: 1,
        kind: "file-attachment",
        filename: "memo.ogg",
        mimeType: "audio/ogg",
        size: 900,
        url: "/files/memo.ogg",
        caption: null,
      },
      agent: "atlas",
    });
    expect(container.querySelector("audio")).toHaveAttribute("src", "/files/memo.ogg");
    expect(container.querySelector("audio")).toHaveAccessibleName("memo.ogg");
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("offers any other file to download, with its size", () => {
    show({
      id: 1,
      kind: "file-attachment",
      filename: "report.pdf",
      mimeType: "application/pdf",
      size: 3 * 1024 * 1024,
      url: "/files/report.pdf",
      caption: null,
    });
    expect(screen.getByRole("link", { name: /report\.pdf/ })).toHaveTextContent("3.0 MB");
  });
});

describe("notes in a transcript", () => {
  it("announces an error status line, with its detail behind a disclosure", async () => {
    show({
      id: 1,
      kind: "status",
      tone: "error",
      content: "Couldn't deliver the message.",
      details: "relay: 503",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't deliver the message.");
    expect(screen.getByText("relay: 503")).not.toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Couldn't deliver the message." }));
    expect(screen.getByText("relay: 503")).toBeVisible();
  });

  it("shows an info status line politely", () => {
    show({ id: 1, kind: "status", tone: "info", content: "The run finished." });
    expect(screen.getByRole("status")).toHaveTextContent("The run finished.");
  });

  it("shows text sent for this page only as it was written", () => {
    show({ id: 1, kind: "local-system", content: "Context: 12k\nTools: 4" });
    expect(screen.getByText(/Context: 12k/).tagName).toBe("PRE");
  });
});
