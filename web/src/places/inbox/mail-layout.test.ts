import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Container queries can't read custom properties, so the Inbox writes the
// width at which the list makes way for the thread out as a number. It must
// be the list's width plus the thread's floor, less a pixel.

function pixels(css: string, name: string): number {
  const value = new RegExp(`${name}:\\s*(\\d+)px;`).exec(css)?.[1];
  if (value === undefined) throw new Error(`${name} is not a pixel token in tokens.css`);
  return Number(value);
}

describe("the Inbox's split width", () => {
  it("is the list's width plus the thread's floor, less a pixel", () => {
    const tokens = readFileSync(new URL("../../styles/tokens.css", import.meta.url), "utf8");
    const place = readFileSync(new URL("./Inbox.svelte", import.meta.url), "utf8");
    const split =
      pixels(tokens, "--layout-mail-list-width") + pixels(tokens, "--layout-mail-thread-min-width");

    const queries = [...place.matchAll(/@container mail \(max-width: (\d+)px\)/g)].map((match) =>
      Number(match[1]),
    );
    expect(queries).toEqual([split - 1]);
  });
});
