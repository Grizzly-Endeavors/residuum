import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen, settle, stubWebSocket } from "../test/component";
import { router } from "../lib/router.svelte";
import { HOME } from "../lib/routes";
import Prose from "./Prose.svelte";

function stubClipboard(writeText: (text: string) => Promise<void>): ReturnType<typeof vi.fn> {
  const spy = vi.fn(writeText);
  Object.defineProperty(navigator, "clipboard", { value: { writeText: spy }, configurable: true });
  return spy;
}

beforeEach(async () => {
  stubWebSocket();
  await router.replacePlace({ kind: "chat", agent: "atlas" });
});

afterEach(async () => {
  await router.replacePlace(HOME);
});

describe("Prose", () => {
  it("renders Markdown with GFM tables and single line breaks", () => {
    const { container } = render(Prose, {
      content: "## Plan\nfirst line\nsecond line\n\n| a | b |\n|---|---|\n| 1 | 2 |",
      agent: "atlas",
    });
    expect(screen.getByRole("heading", { name: "Plan" })).toBeInTheDocument();
    expect(container.querySelector("br")).not.toBeNull();
    expect(screen.getByRole("cell", { name: "2" })).toBeInTheDocument();
  });

  it("shows a caret only while the text is streaming in", async () => {
    const { container, rerender } = render(Prose, {
      content: "The port is set",
      agent: "atlas",
      streaming: true,
    });
    expect(container.querySelector(".prose")).toHaveAttribute("data-caret");

    await rerender({ content: "The port is set twice.", agent: "atlas", streaming: false });
    expect(container.querySelector(".prose")).not.toHaveAttribute("data-caret");
  });

  it("shows no caret by default", () => {
    const { container } = render(Prose, { content: "Done.", agent: "atlas" });
    expect(container.querySelector(".prose")).not.toHaveAttribute("data-caret");
  });

  it("drops script and event handlers from the message", () => {
    const { container } = render(Prose, {
      content: 'hi <script>window.hacked = true</script><img src="x" onerror="alert(1)">',
      agent: "atlas",
    });
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")?.getAttribute("onerror")).toBeNull();
  });

  it("copies a code block without its trailing newline, and says so", async () => {
    const writeText = stubClipboard(() => Promise.resolve());
    render(Prose, { content: "```toml\n[memory]\nlimit = 3\n```", agent: "atlas" });
    const copy = screen.getByRole("button", { name: "Copy code" });

    await userEvent.click(copy);

    expect(writeText).toHaveBeenCalledWith("[memory]\nlimit = 3");
    expect(copy).toHaveTextContent("Copied");
  });

  it("says when the code couldn't be copied", async () => {
    stubClipboard(() => Promise.reject(new Error("denied")));
    render(Prose, { content: "```\nls\n```", agent: "atlas" });
    const copy = screen.getByRole("button", { name: "Copy code" });

    await userEvent.click(copy);
    await settle();

    expect(copy).toHaveTextContent("Couldn't copy");
  });

  it("links a workspace path, and leaves other inline code as code", () => {
    render(Prose, {
      content: "See `team/wiki/index.md`, run `npm test`, and look in `team/wiki`.",
      agent: "atlas",
    });
    const link = screen.getByRole("link", { name: "team/wiki/index.md" });
    expect(link).toHaveAttribute("href", "/agent/atlas?panel=file:team/wiki/index.md");
    expect(screen.getAllByRole("link")).toHaveLength(1);
    expect(screen.getByText("npm test").tagName).toBe("CODE");
  });

  it("opens a path in the panel beside its own agent's place", async () => {
    const openPanel = vi.spyOn(router, "openPanel");
    render(Prose, { content: "It's in `notes/plan.md`.", agent: "atlas" });

    await userEvent.click(screen.getByRole("link", { name: "notes/plan.md" }));

    expect(openPanel).toHaveBeenCalledWith({ kind: "file", path: "notes/plan.md" });
  });

  it("opens another agent's path beside that agent's chat", async () => {
    const openPlace = vi.spyOn(router, "openPlace");
    const openPanel = vi.spyOn(router, "openPanel");
    render(Prose, { content: "It's in `notes/plan.md`.", agent: "scout" });

    await userEvent.click(screen.getByRole("link", { name: "notes/plan.md" }));

    expect(openPanel).not.toHaveBeenCalled();
    expect(openPlace).toHaveBeenCalledWith(
      { kind: "chat", agent: "scout" },
      { panel: { kind: "file", path: "notes/plan.md" } },
    );
  });

  it("says the copy result politely, and renames the button to match", async () => {
    stubClipboard(() => Promise.resolve());
    render(Prose, { content: "```\nls\n```", agent: "atlas" });

    await userEvent.click(screen.getByRole("button", { name: "Copy code" }));

    expect(screen.getByRole("status")).toHaveTextContent("Copied");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("puts a code block in a focusable, named group, and names its Copy button", () => {
    render(Prose, { content: "```toml\nkey = 1\n```\n\n```\nplain\n```", agent: "atlas" });

    const toml = screen.getByRole("group", { name: "Code, toml" });
    expect(toml.tagName).toBe("PRE");
    expect(toml).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("group", { name: "Code" })).toHaveAttribute("tabindex", "0");
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(2);
  });

  it("scrolls a table in a focusable, named group", () => {
    render(Prose, { content: "| a | b |\n|---|---|\n| 1 | 2 |", agent: "atlas" });

    const group = screen.getByRole("group", { name: "Table" });
    expect(group).toHaveAttribute("tabindex", "0");
    expect(group).toContainElement(screen.getByRole("table"));
  });

  it("draws a task list as words and glyphs, with no checkboxes", () => {
    const { container } = render(Prose, {
      content: "- [x] shipped\n- [ ] pending\n- plain",
      agent: "atlas",
    });

    expect(screen.queryByRole("checkbox")).toBeNull();
    const items = screen.getAllByRole("listitem");
    expect(items.map((item) => item.textContent.trim())).toEqual([
      "Done: shipped",
      "To do: pending",
      "plain",
    ]);
    expect(container.querySelectorAll("li.prose-task")).toHaveLength(2);
  });

  it("opens links in a new tab, and keeps a workspace path in the app", () => {
    render(Prose, {
      content: "See [the docs](https://example.com/docs) and `notes/plan.md`.",
      agent: "atlas",
    });

    const docs = screen.getByRole("link", { name: "the docs" });
    expect(docs).toHaveAttribute("target", "_blank");
    expect(docs).toHaveAttribute("rel", "noopener noreferrer");
    expect(screen.getByRole("link", { name: "notes/plan.md" })).not.toHaveAttribute("target");
  });

  it("puts a reply's headings below the page's", () => {
    render(Prose, { content: "# Big\n\n## Middle\n\n###### Small", agent: "atlas" });

    expect(screen.getByRole("heading", { name: "Big", level: 3 })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Middle", level: 4 })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Small", level: 6 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.queryByRole("heading", { level: 2 })).toBeNull();
  });

  it("re-renders when the text changes", async () => {
    const view = render(Prose, { content: "first", agent: "atlas" });
    await view.rerender({ content: "second", agent: "atlas" });
    expect(screen.queryByText("first")).toBeNull();
    expect(screen.getByText("second")).toBeInTheDocument();
  });
});
