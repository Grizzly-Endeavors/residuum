import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Container queries can't read custom properties, so the Inbox writes the
// width at which the list makes way for the thread out as a number: narrower
// than the list's width plus the thread's floor, one shows at a time, and
// from that sum up the list is beside the thread and Back to list is hidden.
// Every query on the place's container, in any file of the place, must sit on
// the same split, or Back and the list could both be gone on a narrow screen.

const PLACE_DIR = new URL("./", import.meta.url);

function pixels(css: string, name: string): number {
  const value = new RegExp(`${name}:\\s*(\\d+)px;`).exec(css)?.[1];
  if (value === undefined) throw new Error(`${name} is not a pixel token in tokens.css`);
  return Number(value);
}

interface Query {
  file: string;
  bound: "max" | "min";
  width: number;
}

function mailQueries(): Query[] {
  return readdirSync(PLACE_DIR)
    .filter((file) => file.endsWith(".svelte"))
    .flatMap((file) => {
      const source = readFileSync(new URL(file, PLACE_DIR), "utf8");
      return [...source.matchAll(/@container mail \((max|min)-width:\s*(\d+)px\)/g)].map(
        (match) => ({
          file,
          bound: match[1] === "max" ? ("max" as const) : ("min" as const),
          width: Number(match[2]),
        }),
      );
    });
}

describe("the Inbox's split width", () => {
  const tokens = readFileSync(new URL("../../styles/tokens.css", import.meta.url), "utf8");
  const split =
    pixels(tokens, "--layout-mail-list-width") + pixels(tokens, "--layout-mail-thread-min-width");

  it("is the list's width plus the thread's floor in every query on the place", () => {
    const queries = mailQueries();
    for (const query of queries) {
      const expected = query.bound === "max" ? split - 1 : split;
      expect(query.width, `${query.file} @container mail (${query.bound}-width)`).toBe(expected);
    }
  });

  it("is written both ways: one for the list making way, one for Back to list hiding", () => {
    const queries = mailQueries();
    expect(queries.some((query) => query.bound === "max")).toBe(true);
    expect(queries.some((query) => query.bound === "min")).toBe(true);
  });
});
