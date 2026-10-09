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

describe("renderMarkdown raw HTML", () => {
  /** What a reader would see of the nodes: their text, with the whitespace between blocks tidied. */
  function words(root: HTMLElement): string {
    return root.textContent.replace(/\s+/g, " ").trim();
  }

  it("drops a style element and its rules, keeping the text around it", () => {
    const root = html("before\n\n<style>.shell { display: none }</style>\n\nafter");
    expect(root.querySelector("style")).toBeNull();
    expect(words(root)).toBe("before after");
  });

  it("drops a form and every control in it, keeping their words", () => {
    const root = html(
      '<form action="https://other.example/steal"><p>Sign in</p><input name="token"><button>Go</button></form>',
    );
    expect(root.querySelector("form, input, button")).toBeNull();
    expect(words(root)).toContain("Sign in");
    expect(words(root)).toContain("Go");
  });

  it.each([
    ["input", "<p>name <input type=text value=hidden> end</p>", "name end"],
    ["button", "<p>press <button>Here</button> now</p>", "press Here now"],
    ["textarea", "<textarea>typed words</textarea>", "typed words"],
    [
      "select and its options",
      "<select><option>One</option><optgroup label=g><option>Two</option></optgroup></select>",
      "OneTwo",
    ],
    [
      "datalist",
      "<p>pick</p><datalist id=d><option value=a>Alpha</option></datalist>",
      "pickAlpha",
    ],
    ["fieldset and legend", "<fieldset><legend>Legend</legend>body</fieldset>", "Legendbody"],
    ["output and label", "<label for=x>Name</label><output>42</output>", "Name42"],
  ])("drops %s, keeping its text", (_name, markup, text) => {
    const root = html(markup);
    expect(
      root.querySelector("input, button, textarea, select, option, optgroup, datalist"),
    ).toBeNull();
    expect(root.querySelector("fieldset, legend, output, label")).toBeNull();
    expect(words(root).replace(/ /g, "")).toBe(text.replace(/ /g, ""));
  });

  it.each([
    ["link", '<link rel="stylesheet" href="https://other.example/a.css"><p>text</p>'],
    ["meta", '<meta http-equiv="refresh" content="0;url=https://other.example"><p>text</p>'],
    ["base", '<base href="https://other.example/"><p>text</p>'],
  ])("drops a %s element", (_name, markup) => {
    const root = html(markup);
    expect(root.querySelector("link, meta, base")).toBeNull();
    expect(words(root)).toBe("text");
  });

  it("drops frames and plug-ins, keeping the fallback text an object carries", () => {
    const root = html(
      '<iframe src="https://other.example">frame</iframe><object data="https://other.example/a.swf">plug-in fallback</object><embed src="https://other.example/a.swf"><p>text</p>',
    );
    expect(root.querySelector("iframe, object, embed")).toBeNull();
    expect(words(root)).toContain("plug-in fallback");
    expect(words(root)).toContain("text");
  });

  it("drops svg and math, and the script an svg link carries", () => {
    const root = html(
      '<svg xmlns="http://www.w3.org/2000/svg"><a href="javascript:alert(1)"><rect/></a></svg><math><mi>x</mi></math><p>text</p>',
    );
    expect(root.querySelector("svg, math, a")).toBeNull();
    expect(words(root)).toBe("text");
  });

  it("drops a script element", () => {
    const root = html("<script>window.hacked = true</script><p>text</p>");
    expect(root.querySelector("script")).toBeNull();
    expect(words(root)).toBe("text");
  });

  it("drops a dialog and a marquee, keeping their text", () => {
    const root = html("<dialog open>covering</dialog><marquee>moving</marquee>");
    expect(root.querySelector("dialog, marquee")).toBeNull();
    expect(words(root)).toBe("coveringmoving");
  });

  it("drops every inline style, so nothing can be placed over the app", () => {
    const root = html(
      '<div style="position:fixed;inset:0;background:red">cover</div><p style="color:red">red</p><img src="https://example.com/i.png" style="position:absolute" alt="pic">',
    );
    expect(root.querySelector("[style]")).toBeNull();
    expect(words(root)).toBe("coverred");
    expect(root.querySelector("img")?.getAttribute("alt")).toBe("pic");
  });

  it("drops the popover attribute and keeps the element's text", () => {
    const root = html("<div popover>overlay</div>");
    expect(root.querySelector("[popover]")).toBeNull();
    expect(words(root)).toBe("overlay");
  });

  it("drops a button made to look like a Copy button", () => {
    const root = html("<button type=button class=prose-copy data-copy-code>Copy</button>");
    expect(root.querySelector("button")).toBeNull();
  });

  it("keeps the task glyph, its words and its classes, which need no inline style", () => {
    const root = html("- [x] shipped\n- [ ] pending");
    const checks = [...root.querySelectorAll<HTMLElement>("li.prose-task > .prose-check")];
    expect(checks.map((check) => [check.dataset.done, check.textContent])).toEqual([
      ["true", "Done: "],
      ["false", "To do: "],
    ]);
    expect(root.querySelector("[style]")).toBeNull();
    expect(root.querySelector("input")).toBeNull();
  });

  it("still gives each code block its own Copy button, after the sanitizer", () => {
    const root = html("```\none\n```\n\n```\ntwo\n```");
    const buttons = root.querySelectorAll<HTMLButtonElement>(
      ".prose-code > button[data-copy-code]",
    );
    expect(buttons).toHaveLength(2);
    for (const button of buttons) {
      expect(button.type).toBe("button");
      expect(button.getAttribute("aria-label")).toBe("Copy code");
      expect(button.textContent).toBe("Copy");
      expect(button.previousElementSibling?.tagName).toBe("PRE");
    }
  });

  it("renders ordinary Markdown and harmless HTML as it always has", () => {
    const root = html(
      [
        "Some **bold**, _em_, `code` and a [link](https://example.com/x).",
        "",
        "> quote",
        "",
        "1. one",
        "2. two",
        "",
        '<div class="note" id="n">raw <span>html</span></div>',
        "",
        "<details><summary>More</summary>hidden <b>bold</b></details>",
      ].join("\n"),
    );
    expect(root.innerHTML).toBe(
      [
        '<p>Some <strong>bold</strong>, <em>em</em>, <code>code</code> and a <a href="https://example.com/x" target="_blank" rel="noopener noreferrer">link</a>.</p>',
        "<blockquote>",
        "<p>quote</p>",
        "</blockquote>",
        "<ol>",
        "<li>one</li>",
        "<li>two</li>",
        "</ol>",
        '<div class="note" id="n">raw <span>html</span></div>' +
          "<details><summary>More</summary>hidden <b>bold</b></details>",
      ].join("\n"),
    );
  });
});
