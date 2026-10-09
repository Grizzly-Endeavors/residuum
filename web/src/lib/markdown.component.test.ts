import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

/** The rendered nodes in a container, so a test can query them. */
function html(content: string, pathHref?: (path: string) => string): HTMLElement {
  const root = document.createElement("div");
  root.append(renderMarkdown(content, { pathHref }));
  return root;
}

describe("renderMarkdown headings", () => {
  it("moves every level two below the page's own, stopping at h6", () => {
    const root = html("# a\n\n## b\n\n### c\n\n#### d\n\n##### e\n\n###### f");
    const tags = [...root.querySelectorAll("h1, h2, h3, h4, h5, h6")].map((h) => h.tagName);
    expect(tags).toEqual(["H3", "H4", "H5", "H6", "H6", "H6"]);
  });

  it("moves headings written as raw HTML too, keeping their text and markup", () => {
    const root = html('<h1 id="top">Plan <em>now</em></h1>');
    const heading = root.querySelector("h3");
    expect(heading?.innerHTML).toBe("Plan <em>now</em>");
    expect(heading?.id).toBe("top");
    expect(root.querySelector("h1")).toBeNull();
  });
});

describe("renderMarkdown tables", () => {
  it("wraps each table in one focusable, named group", () => {
    const root = html("| a | b |\n|---|---|\n| 1 | 2 |\n\n> | c |\n> |---|\n> | 3 |");
    const groups = root.querySelectorAll<HTMLElement>(".prose-table");
    expect(groups).toHaveLength(2);
    for (const group of groups) {
      expect(group.getAttribute("role")).toBe("group");
      expect(group.getAttribute("aria-label")).toBe("Table");
      expect(group.tabIndex).toBe(0);
      expect(group.querySelectorAll(":scope > table")).toHaveLength(1);
    }
  });
});

describe("renderMarkdown links", () => {
  it("opens an external link in a new tab without handing over the opener", () => {
    const link = html("[docs](https://example.com/a)").querySelector("a");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
  });

  it("does the same for an autolink and for a link written as raw HTML", () => {
    const root = html('<https://example.com> and <a href="/agent/atlas">atlas</a>');
    const links = [...root.querySelectorAll("a")];
    expect(links).toHaveLength(2);
    for (const link of links) expect(link.getAttribute("target")).toBe("_blank");
  });

  it("leaves a workspace path link and an in-page fragment alone", () => {
    const root = html("See `notes/plan.md` and [below](#end).", (path) => `/files/${path}`);
    const path = root.querySelector("a[data-path]");
    expect(path?.getAttribute("href")).toBe("/files/notes/plan.md");
    expect(path?.hasAttribute("target")).toBe(false);
    const fragment = root.querySelector('a[href="#end"]');
    expect(fragment?.hasAttribute("target")).toBe(false);
  });

  it("still drops a script URL", () => {
    const root = html("[bad](javascript:alert(1))");
    expect(root.querySelector("a[href^='javascript']")).toBeNull();
  });
});

describe("renderMarkdown task lists", () => {
  it("leads a task with its state in words and no checkbox", () => {
    const root = html("- [x] shipped\n- [ ] pending");
    expect(root.querySelector("input")).toBeNull();
    const checks = [...root.querySelectorAll<HTMLElement>("li.prose-task > .prose-check")];
    expect(checks.map((check) => [check.dataset.done, check.textContent])).toEqual([
      ["true", "Done: "],
      ["false", "To do: "],
    ]);
  });

  it("marks a task in a loose list and in a nested list", () => {
    const loose = html("- [ ] one\n\n- [x] two");
    expect(loose.querySelectorAll("li.prose-task")).toHaveLength(2);
    const nested = html("- parent\n  - [x] child");
    expect(nested.querySelectorAll("li.prose-task")).toHaveLength(1);
    expect(nested.querySelector("li:not(.prose-task)")?.textContent).toContain("parent");
  });

  it("leaves a list item that only looks like a task as plain text", () => {
    const root = html("- [maybe] later");
    expect(root.querySelector(".prose-check")).toBeNull();
    expect(root.querySelector("li")?.textContent).toBe("[maybe] later");
  });
});

describe("renderMarkdown code blocks", () => {
  it("makes a block focusable and named by its language, with a named Copy button", () => {
    const root = html("```rust\nfn main() {}\n```");
    const pre = root.querySelector("pre");
    expect(pre?.getAttribute("tabindex")).toBe("0");
    expect(pre?.getAttribute("role")).toBe("group");
    expect(pre?.getAttribute("aria-label")).toBe("Code, rust");
    const copy = root.querySelector("button[data-copy-code]");
    expect(copy?.getAttribute("aria-label")).toBe("Copy code");
    expect(copy?.textContent).toBe("Copy");
  });

  it("escapes a language that carries markup", () => {
    const root = html('```"><img src=x onerror=alert(1)>\ncode\n```');
    expect(root.querySelector("img")).toBeNull();
  });
});
