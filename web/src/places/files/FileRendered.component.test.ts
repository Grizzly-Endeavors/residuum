import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { HOME } from "../../lib/routes";
import { render, screen } from "../../test/component";
import FileRendered from "./FileRendered.svelte";

const ATLAS = { agent: "atlas", scope: "agent" } as const;
const TEAM = { agent: null, scope: "team" } as const;

describe("FileRendered in text mode", () => {
  it("shows the text as it is, with its Markdown characters left alone", () => {
    render(FileRendered, {
      content: "*not em* and # not a heading\n",
      path: "notes/readme.txt",
      source: ATLAS,
      mode: "text",
    });

    expect(screen.getByText("*not em* and # not a heading").tagName).toBe("PRE");
    expect(screen.queryByRole("heading")).toBeNull();
  });
});

describe("FileRendered in Markdown mode", () => {
  it("renders the Markdown, emphasis included", () => {
    render(FileRendered, {
      content: "# Title\n\n*lean* text\n",
      path: "notes/plan.md",
      source: ATLAS,
      mode: "markdown",
    });

    expect(screen.getByRole("heading", { name: "Title" })).toBeTruthy();
    expect(screen.getByText("lean").tagName).toBe("EM");
  });
});

describe("a relative link in a team file", () => {
  beforeEach(async () => {
    await router.replacePlace({ kind: "shared-files" });
  });

  afterEach(async () => {
    await router.replacePlace(HOME);
  });

  it("opens the linked file in the Shared files panel", async () => {
    const openPanel = vi.spyOn(router, "openPanel");
    render(FileRendered, {
      content: "Read the [rules](../AGENTS.md).\n",
      path: "wiki/index.md",
      source: TEAM,
      mode: "markdown",
    });

    await userEvent.click(screen.getByRole("link", { name: "rules" }));

    expect(openPanel).toHaveBeenCalledWith({ kind: "file", path: "AGENTS.md", agent: null });
  });
});
