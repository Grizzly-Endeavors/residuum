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
    const copy = screen.getByRole("button", { name: "Copy" });

    await userEvent.click(copy);

    expect(writeText).toHaveBeenCalledWith("[memory]\nlimit = 3");
    expect(copy).toHaveTextContent("Copied");
  });

  it("says when the code couldn't be copied", async () => {
    stubClipboard(() => Promise.reject(new Error("denied")));
    render(Prose, { content: "```\nls\n```", agent: "atlas" });
    const copy = screen.getByRole("button", { name: "Copy" });

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

  it("re-renders when the text changes", async () => {
    const view = render(Prose, { content: "first", agent: "atlas" });
    await view.rerender({ content: "second", agent: "atlas" });
    expect(screen.queryByText("first")).toBeNull();
    expect(screen.getByText("second")).toBeInTheDocument();
  });
});
